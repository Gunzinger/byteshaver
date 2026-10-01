//! EXIF payload parsing, policy-driven re-serialization and orientation
//! extraction, built on `kamadak-exif` (imported as `exif`).
//!
//! # Payload convention
//!
//! All functions in this module expect the **raw TIFF byte stream**: the
//! content of a JPEG APP1 segment after its `b"Exif\0\0"` header, the body
//! of a PNG `eXIf` chunk, the WebP `EXIF` chunk body after its optional
//! `b"Exif\0\0"` prefix (see [`crate::metadata::normalize_exif_payload`]).
//! Nothing here accepts or produces the `Exif\0\0` marker prefix.

use exif::experimental::Writer;
use exif::{Context, In, Reader, Tag};

use crate::metadata::ImageMetadata;
use crate::metadata::policy::{ExifPolicy, Ifd, TagSelector};

/// Small typed summary of the most-visited EXIF fields of a photo, for
/// front-ends (GUI file-table columns, job-API consumers) so they do not
/// need a `kamadak-exif` dependency of their own. Every field is `None`
/// when the source does not carry it.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExifSummary {
    /// `Make` and `Model` joined with a single space (`None` when neither
    /// tag is present).
    pub camera: Option<String>,
    /// `DateTimeOriginal` verbatim (`YYYY:MM:DD HH:MM:SS` — this exact
    /// spelling sorts chronologically as a plain string).
    pub date_time_original: Option<String>,
    /// ISO sensitivity (`ISOSpeed`).
    pub iso: Option<u32>,
    /// Exposure time rendered camera-style: `1/125`, `1/2`, `2s`.
    pub exposure: Option<String>,
}

/// Reads an [`ExifSummary`] straight from a container file on disk
/// (JPEG/PNG/WebP/HEIF/TIFF — whatever `kamadak-exif` parses as a
/// container). Returns `None` when the file is unreadable, unparsable or
/// carries no EXIF at all; never panics.
#[must_use]
pub fn read_summary(path: &std::path::Path) -> Option<ExifSummary> {
    let bytes = std::fs::read(path).ok()?;
    let parsed = parse_with_container(&bytes)?;
    let text = |tag: Tag| -> Option<String> {
        match &parsed.get_field(tag, In::PRIMARY)?.value {
            exif::Value::Ascii(values) => values
                .first()
                .map(|raw| String::from_utf8_lossy(raw).trim().to_string())
                .filter(|text| !text.is_empty()),
            _ => None,
        }
    };
    let camera = match (text(Tag::Make), text(Tag::Model)) {
        (Some(make), Some(model)) => Some(format!("{make} {model}")),
        (make, model) => make.or(model),
    };
    let exposure = parsed
        .get_field(Tag::ExposureTime, In::PRIMARY)
        .and_then(|field| match &field.value {
            exif::Value::Rational(values) => values.first().copied(),
            _ => None,
        })
        .map(format_exposure);
    Some(ExifSummary {
        camera,
        date_time_original: text(Tag::DateTimeOriginal),
        iso: parsed
            .get_field(Tag::ISOSpeed, In::PRIMARY)
            .and_then(|field| field.value.get_uint(0)),
        exposure,
    })
}

/// Renders an exposure-time rational camera-style (`1/125`, `1/2`, `2s`).
fn format_exposure(rational: exif::Rational) -> String {
    if rational.denom == 0 {
        return format!("{}?", rational.num);
    }
    if rational.num == 0 {
        return "0".to_string();
    }
    if rational.denom == 1 {
        return format!("{}s", rational.num);
    }
    format!("{}/{}", rational.num, rational.denom)
}

/// Parses a raw TIFF payload into kamadak's [`Exif`][exif::Exif] structure.
pub fn parse(payload: &[u8]) -> Result<exif::Exif, exif::Error> {
    Reader::new().read_raw(payload.to_vec())
}

/// Parses the EXIF attributes of a whole container file (TIFF and friends;
/// kamadak also handles JPEG/PNG/WebP/HEIF containers).
///
/// Used by the extraction fallbacks; returns `None` when the container does
/// not carry EXIF or cannot be parsed.
#[must_use]
pub fn parse_with_container(bytes: &[u8]) -> Option<exif::Exif> {
    Reader::new()
        .read_from_container(&mut std::io::Cursor::new(bytes))
        .ok()
}

/// Returns the EXIF Orientation value (1–8) carried by the parsed fields.
///
/// `None` if the payload has no usable Orientation tag.
#[must_use]
pub fn orientation_value(exif: &exif::Exif) -> Option<u8> {
    exif.get_field(Tag::Orientation, In::PRIMARY)
        .and_then(|field| field.value.get_uint(0))
        .and_then(|value| u8::try_from(value).ok())
        .filter(|value| (1..=8).contains(value))
}

/// Parses a raw TIFF payload and returns its Orientation value (1–8).
#[must_use]
pub fn orientation_from_payload(payload: &[u8]) -> Option<u8> {
    parse(payload)
        .ok()
        .and_then(|exif| orientation_value(&exif))
}

/// Resolves the EXIF policy against the metadata of a source image.
///
/// This is the single function encoders/pipeline rely on. Rules:
/// 1. No source EXIF → `None` regardless of policy (nothing is synthesized).
/// 2. [`ExifPolicy::Strip`] → `None`.
/// 3. [`ExifPolicy::Keep`] → the original payload, verbatim.
/// 4. [`ExifPolicy::FilterExcept`]/[`ExifPolicy::KeepOnly`] → a re-serialized
///    payload holding the surviving fields; on writer failure a per-file
///    warning is printed and the original payload is kept verbatim (never
///    silently lose everything).
#[must_use]
pub fn resolve(policy: &ExifPolicy, meta: &ImageMetadata) -> Option<Vec<u8>> {
    let payload = meta.exif.as_ref()?;
    match policy {
        ExifPolicy::Strip => None,
        ExifPolicy::Keep => Some(payload.clone()),
        ExifPolicy::FilterExcept(_) | ExifPolicy::KeepOnly(_) => {
            match parse(payload)
                .map_err(TransformError::from)
                .and_then(|parsed| transform(&parsed, policy))
            {
                Ok(filtered) => Some(filtered),
                Err(message) => {
                    println!(
                        "Warning: could not re-serialize EXIF ({message}); keeping metadata verbatim"
                    );
                    Some(payload.clone())
                }
            }
        }
    }
}

/// Errors of the filter/serialize path (kept simple: string messages).
#[derive(Debug)]
pub struct TransformError(String);

impl std::fmt::Display for TransformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TransformError {}

impl From<exif::Error> for TransformError {
    fn from(err: exif::Error) -> Self {
        TransformError(err.to_string())
    }
}

/// Filters the fields of `exif` according to `policy` and re-serializes the
/// surviving fields into a raw TIFF payload.
///
/// Endianness of the source is preserved. Structural pointer fields
/// (ExifIFDPointer etc.) are re-synthesized by the writer.
pub fn transform(exif: &exif::Exif, policy: &ExifPolicy) -> Result<Vec<u8>, TransformError> {
    let (ExifPolicy::FilterExcept(_) | ExifPolicy::KeepOnly(_)) = policy else {
        return Err(TransformError(
            "transform expects a filter policy (FilterExcept/KeepOnly)".to_string(),
        ));
    };

    let mut seen: Vec<(In, Tag)> = Vec::new();
    let mut writer = Writer::new();
    let mut kept = 0u64;
    let mut dropped = 0u64;
    for field in exif.fields() {
        // the writer rejects duplicate fields; keep the first occurrence
        let key = (field.ifd_num, field.tag);
        if seen.contains(&key) {
            continue;
        }
        if field_is_dropped(field, policy) {
            dropped += 1;
        } else {
            seen.push(key);
            writer.push_field(field);
            kept += 1;
        }
    }

    let mut serialized = std::io::Cursor::new(Vec::new());
    writer
        .write(&mut serialized, exif.little_endian())
        .map_err(TransformError::from)?;
    println!(
        "exif: {kept} kept, {dropped} dropped ({})",
        policy.describe()
    );
    Ok(serialized.into_inner())
}

/// Re-serializes every field of `exif` into a raw TIFF payload.
///
/// Used by the extraction fallbacks (e.g. TIFF containers where the decoder
/// exposes no raw payload): the result is a compact, self-contained payload
/// instead of the whole original file.
pub fn serialize_all(exif: &exif::Exif) -> Result<Vec<u8>, TransformError> {
    let mut writer = Writer::new();
    let mut seen: Vec<(In, Tag)> = Vec::new();
    for field in exif.fields() {
        let key = (field.ifd_num, field.tag);
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        writer.push_field(field);
    }
    let mut serialized = std::io::Cursor::new(Vec::new());
    writer
        .write(&mut serialized, exif.little_endian())
        .map_err(TransformError::from)?;
    Ok(serialized.into_inner())
}

/// Whether the field is removed under the given filter policy:
/// [`ExifPolicy::FilterExcept`] drops fields matching any selector,
/// [`ExifPolicy::KeepOnly`] keeps fields matching any selector.
fn field_is_dropped(field: &exif::Field, policy: &ExifPolicy) -> bool {
    let selectors = match policy {
        ExifPolicy::FilterExcept(selectors) => selectors,
        ExifPolicy::KeepOnly(selectors) => {
            return !selectors
                .iter()
                .any(|selector| field_matches_selector(field, selector));
        }
        _ => return false,
    };
    selectors
        .iter()
        .any(|selector| field_matches_selector(field, selector))
}

fn field_matches_selector(field: &exif::Field, selector: &TagSelector) -> bool {
    match selector {
        TagSelector::Name(name) => resolve_name(name).is_some_and(|tag| tag == field.tag),
        TagSelector::Ifd(ifd) => field_in_ifd(field, *ifd),
        TagSelector::TagNum(tag_num, ifd) => {
            field.tag.number() == *tag_num && field_in_ifd_context(field, *ifd)
        }
    }
}

/// Resolves a tag name (case-insensitive) from the static table onto a
/// kamadak tag value.
#[must_use]
pub fn resolve_name(name: &str) -> Option<Tag> {
    crate::metadata::policy::KNOWN_TAGS
        .iter()
        .find(|&&(known, _, _)| known.eq_ignore_ascii_case(name))
        .map(|&(_, ifd, tag_num)| tag_for_group(ifd, tag_num))
}
/// Builds a kamadak tag from the IFD group and tag number of the static table.
#[must_use]
pub fn tag_for_group(ifd: Ifd, tag_num: u16) -> Tag {
    match ifd {
        Ifd::Primary | Ifd::Thumbnail => Tag(Context::Tiff, tag_num),
        Ifd::Exif => Tag(Context::Exif, tag_num),
        Ifd::Gps => Tag(Context::Gps, tag_num),
        Ifd::Interop => Tag(Context::Interop, tag_num),
    }
}

fn field_in_ifd(field: &exif::Field, ifd: Ifd) -> bool {
    match ifd {
        Ifd::Primary => field.tag.context() == Context::Tiff && field.ifd_num == In::PRIMARY,
        Ifd::Thumbnail => field.tag.context() == Context::Tiff && field.ifd_num == In::THUMBNAIL,
        Ifd::Exif | Ifd::Gps | Ifd::Interop => field_in_ifd_context(field, ifd),
    }
}

fn field_in_ifd_context(field: &exif::Field, ifd: Ifd) -> bool {
    let context = match ifd {
        Ifd::Primary | Ifd::Thumbnail => Context::Tiff,
        Ifd::Exif => Context::Exif,
        Ifd::Gps => Context::Gps,
        Ifd::Interop => Context::Interop,
    };
    if field.tag.context() != context {
        return false;
    }
    match ifd {
        Ifd::Primary => field.ifd_num == In::PRIMARY,
        Ifd::Thumbnail => field.ifd_num == In::THUMBNAIL,
        // sub-IFDs share the top-level IFD number of their pointer
        Ifd::Exif | Ifd::Gps | Ifd::Interop => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Builds a bare TIFF file (valid container for kamadak) carrying the
    /// summary tags via the crate's own writer.
    fn write_tiff_fixture(path: &std::path::Path, with_exif_sub_ifd: bool) {
        let fields = [
            exif::Field {
                tag: Tag::Make,
                ifd_num: In::PRIMARY,
                value: exif::Value::Ascii(vec![b"TestCam\0".to_vec()]),
            },
            exif::Field {
                tag: Tag::Model,
                ifd_num: In::PRIMARY,
                value: exif::Value::Ascii(vec![b"Byteshaver 9000\0".to_vec()]),
            },
        ];
        let sub_ifd = [
            exif::Field {
                tag: Tag::DateTimeOriginal,
                ifd_num: In::PRIMARY,
                value: exif::Value::Ascii(vec![b"2024:05:01 12:34:56\0".to_vec()]),
            },
            exif::Field {
                tag: Tag::ISOSpeed,
                ifd_num: In::PRIMARY,
                value: exif::Value::Short(vec![200]),
            },
            exif::Field {
                tag: Tag::ExposureTime,
                ifd_num: In::PRIMARY,
                value: exif::Value::Rational(vec![exif::Rational {
                    num: 1,
                    denom: 125,
                }]),
            },
        ];
        let mut writer = Writer::new();
        for field in &fields {
            writer.push_field(field);
        }
        if with_exif_sub_ifd {
            // Exif-IFD context tags: the writer synthesizes the pointer
            for field in &sub_ifd {
                writer.push_field(field);
            }
        }
        let mut serialized = Cursor::new(Vec::new());
        writer
            .write(&mut serialized, true)
            .expect("fixture writer");
        std::fs::write(path, serialized.into_inner()).expect("write fixture");
    }

    #[test]
    fn read_summary_extracts_camera_date_iso_exposure() {
        let dir = std::env::temp_dir().join(format!("byteshaver-exif-summary-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("photo.tif");
        write_tiff_fixture(&file, true);

        let summary = read_summary(&file).expect("summary of an EXIF-bearing fixture");
        assert_eq!(summary.camera.as_deref(), Some("TestCam Byteshaver 9000"));
        assert_eq!(
            summary.date_time_original.as_deref(),
            Some("2024:05:01 12:34:56")
        );
        assert_eq!(summary.iso, Some(200));
        assert_eq!(summary.exposure.as_deref(), Some("1/125"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_summary_handles_partial_and_absent_metadata() {
        let dir = std::env::temp_dir().join(format!("byteshaver-exif-partial-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("partial.tif");
        write_tiff_fixture(&file, false);

        let summary = read_summary(&file).expect("parses");
        assert_eq!(summary.camera.as_deref(), Some("TestCam Byteshaver 9000"));
        assert_eq!(summary.date_time_original, None);
        assert_eq!(summary.iso, None);
        assert_eq!(summary.exposure, None);

        // no EXIF at all / no file at all → None, never panic
        let plain = dir.join("plain.png");
        std::fs::write(&plain, [0u8; 16]).expect("write junk");
        assert_eq!(read_summary(&plain), None);
        assert_eq!(read_summary(&dir.join("missing.tif")), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exposure_rendering_covers_the_camera_styles() {
        let rational = |num: u32, denom: u32| exif::Rational { num, denom };
        assert_eq!(format_exposure(rational(1, 125)), "1/125");
        assert_eq!(format_exposure(rational(1, 2)), "1/2");
        assert_eq!(format_exposure(rational(2, 1)), "2s");
        assert_eq!(format_exposure(rational(3, 10)), "3/10");
        assert_eq!(format_exposure(rational(0, 100)), "0");
        assert_eq!(format_exposure(rational(5, 0)), "5?");
    }
}
