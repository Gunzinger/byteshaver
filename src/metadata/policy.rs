//! EXIF policy types: what happens to EXIF metadata during conversion.
//!
//! This module is dependency-free (no `kamadak-exif`) so that the
//! [`ConversionConfig`][crate::config::ConversionConfig] can always carry an
//! [`ExifPolicy`]; the tag resolution table below stores plain tag numbers
//! that [`crate::metadata::exif`] maps onto `exif::Tag` values.

use std::fmt;

/// The IFD (sub-directory) a tag lives in, as used by CLI selectors.
///
/// Maps onto the tag *context* and the top-level IFD number of the
/// EXIF/TIFF structure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Ifd {
    /// TIFF attributes of the primary image (IFD0).
    Primary,
    /// TIFF attributes of the thumbnail (IFD1).
    Thumbnail,
    /// Attributes of the Exif sub-IFD (e.g. DateTimeOriginal).
    Exif,
    /// Attributes of the GPS sub-IFD.
    Gps,
    /// Attributes of the interoperability sub-IFD.
    Interop,
}

impl Ifd {
    /// All IFDs in CLI listing order.
    pub const ALL: [Ifd; 5] = [
        Ifd::Primary,
        Ifd::Thumbnail,
        Ifd::Exif,
        Ifd::Gps,
        Ifd::Interop,
    ];

    /// CLI names accepted for this IFD (first one is the canonical name).
    #[must_use]
    pub fn cli_names(self) -> &'static [&'static str] {
        match self {
            Ifd::Primary => &["ifd0", "primary", "tiff"],
            Ifd::Thumbnail => &["ifd1", "thumbnail"],
            Ifd::Exif => &["exif"],
            Ifd::Gps => &["gps"],
            Ifd::Interop => &["interop", "interoperability"],
        }
    }

    /// Resolves a CLI name (case-insensitive) onto an IFD.
    #[must_use]
    pub fn from_cli_name(name: &str) -> Option<Self> {
        let lowered = name.to_ascii_lowercase();
        Ifd::ALL.into_iter().find(|ifd| {
            ifd.cli_names()
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(&lowered))
        })
    }
}

impl fmt::Display for Ifd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.cli_names()[0])
    }
}

/// Selector for a single EXIF tag (or tag group) in `--exif-only`/`--exif-except` lists.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TagSelector {
    /// A named tag, resolved via the table of common tags
    /// (e.g. `GPSInfo`, `DateTimeOriginal`, `Orientation`).
    Name(String),
    /// A whole IFD: every field of this group matches.
    Ifd(Ifd),
    /// Numeric tag fallback (e.g. `0x8825`) restricted to an IFD group.
    TagNum(u16, Ifd),
}

impl fmt::Display for TagSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TagSelector::Name(name) => f.write_str(name),
            TagSelector::Ifd(ifd) => write!(f, "{ifd}"),
            TagSelector::TagNum(num, ifd) => write!(f, "0x{num:04x} ({ifd})"),
        }
    }
}

/// How EXIF metadata of the source is treated during conversion.
///
/// The default is [`ExifPolicy::Strip`] (privacy-safe: no metadata leaks
/// into outputs unless explicitly requested).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ExifPolicy {
    /// Remove all EXIF from outputs (DEFAULT); any orientation transform is
    /// baked into the pixels so outputs never appear sideways.
    #[default]
    Strip,
    /// Copy EXIF verbatim into outputs where the container supports it;
    /// pixels are left untouched (viewers rotate via the Orientation tag).
    Keep,
    /// Keep all EXIF except the listed tags (e.g. `--exif-except gps,GPSInfo`).
    FilterExcept(Vec<TagSelector>),
    /// Keep only the listed tags (e.g. `--exif-only DateTimeOriginal`).
    KeepOnly(Vec<TagSelector>),
}

impl ExifPolicy {
    /// Whether this policy wants to preserve (some) EXIF metadata.
    #[must_use]
    pub fn wants_metadata(&self) -> bool {
        !matches!(self, ExifPolicy::Strip)
    }

    /// Human-readable summary used in log lines (e.g. `filter - gps*`).
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            ExifPolicy::Strip => "strip".to_string(),
            ExifPolicy::Keep => "keep".to_string(),
            ExifPolicy::FilterExcept(selectors) => {
                format!("except {}", format_selectors(selectors))
            }
            ExifPolicy::KeepOnly(selectors) => format!("only {}", format_selectors(selectors)),
        }
    }
}

fn format_selectors(selectors: &[TagSelector]) -> String {
    selectors
        .iter()
        .map(|selector| selector.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// The table of recognized tag names for `--exif-only`/`--exif-except`
/// selectors and `--exif-list-tags`: `(name, IFD group, tag number)`.
///
/// Covers the common subset of the EXIF spec; anything else can be selected
/// numerically (`0x8825`). Tag numbers follow the EXIF specification
/// (JEITA CP-3451) and match the `kamadak-exif` tag constants.
pub const KNOWN_TAGS: &[(&str, Ifd, u16)] = &[
    // -- IFD0 (TIFF) attributes ------------------------------------------
    ("ImageDescription", Ifd::Primary, 0x010e),
    ("Make", Ifd::Primary, 0x010f),
    ("Model", Ifd::Primary, 0x0110),
    ("Orientation", Ifd::Primary, 0x0112),
    ("XResolution", Ifd::Primary, 0x011a),
    ("YResolution", Ifd::Primary, 0x011b),
    ("ResolutionUnit", Ifd::Primary, 0x0128),
    ("Software", Ifd::Primary, 0x0131),
    ("DateTime", Ifd::Primary, 0x0132),
    ("Artist", Ifd::Primary, 0x013b),
    ("WhitePoint", Ifd::Primary, 0x013e),
    ("PrimaryChromaticities", Ifd::Primary, 0x013f),
    ("YCbCrCoefficients", Ifd::Primary, 0x0211),
    ("YCbCrSubSampling", Ifd::Primary, 0x0212),
    ("YCbCrPositioning", Ifd::Primary, 0x0213),
    ("ReferenceBlackWhite", Ifd::Primary, 0x0214),
    ("Copyright", Ifd::Primary, 0x8298),
    ("ExifOffset", Ifd::Primary, 0x8769),
    ("GPSInfo", Ifd::Primary, 0x8825),
    // -- Exif sub-IFD ------------------------------------------------------
    ("ExposureTime", Ifd::Exif, 0x829a),
    ("FNumber", Ifd::Exif, 0x829d),
    ("ExposureProgram", Ifd::Exif, 0x8822),
    ("ISOSpeedRatings", Ifd::Exif, 0x8827),
    ("SensitivityType", Ifd::Exif, 0x8830),
    ("ExifVersion", Ifd::Exif, 0x9000),
    ("DateTimeOriginal", Ifd::Exif, 0x9003),
    ("DateTimeDigitized", Ifd::Exif, 0x9004),
    ("OffsetTime", Ifd::Exif, 0x9010),
    ("OffsetTimeOriginal", Ifd::Exif, 0x9011),
    ("OffsetTimeDigitized", Ifd::Exif, 0x9012),
    ("ShutterSpeedValue", Ifd::Exif, 0x9201),
    ("ApertureValue", Ifd::Exif, 0x9202),
    ("BrightnessValue", Ifd::Exif, 0x9203),
    ("ExposureBiasValue", Ifd::Exif, 0x9204),
    ("MaxApertureValue", Ifd::Exif, 0x9205),
    ("SubjectDistance", Ifd::Exif, 0x9206),
    ("MeteringMode", Ifd::Exif, 0x9207),
    ("LightSource", Ifd::Exif, 0x9208),
    ("Flash", Ifd::Exif, 0x9209),
    ("FocalLength", Ifd::Exif, 0x920a),
    ("SubjectArea", Ifd::Exif, 0x9214),
    ("UserComment", Ifd::Exif, 0x9286),
    ("SubSecTime", Ifd::Exif, 0x9290),
    ("SubSecTimeOriginal", Ifd::Exif, 0x9291),
    ("SubSecTimeDigitized", Ifd::Exif, 0x9292),
    ("FlashpixVersion", Ifd::Exif, 0xa000),
    ("ColorSpace", Ifd::Exif, 0xa001),
    ("ExifImageWidth", Ifd::Exif, 0xa002),
    ("ExifImageHeight", Ifd::Exif, 0xa003),
    ("RelatedSoundFile", Ifd::Exif, 0xa004),
    ("InteropOffset", Ifd::Exif, 0xa005),
    ("FlashEnergy", Ifd::Exif, 0xa20b),
    ("FocalPlaneXResolution", Ifd::Exif, 0xa20e),
    ("FocalPlaneYResolution", Ifd::Exif, 0xa20f),
    ("FocalPlaneResolutionUnit", Ifd::Exif, 0xa210),
    ("SubjectLocation", Ifd::Exif, 0xa214),
    ("ExposureIndex", Ifd::Exif, 0xa215),
    ("SensingMethod", Ifd::Exif, 0xa217),
    ("FileSource", Ifd::Exif, 0xa300),
    ("SceneType", Ifd::Exif, 0xa301),
    ("CFAPattern", Ifd::Exif, 0xa302),
    ("CustomRendered", Ifd::Exif, 0xa401),
    ("ExposureMode", Ifd::Exif, 0xa402),
    ("WhiteBalance", Ifd::Exif, 0xa403),
    ("DigitalZoomRatio", Ifd::Exif, 0xa404),
    ("FocalLengthIn35mmFilm", Ifd::Exif, 0xa405),
    ("SceneCaptureType", Ifd::Exif, 0xa406),
    ("GainControl", Ifd::Exif, 0xa407),
    ("Contrast", Ifd::Exif, 0xa408),
    ("Saturation", Ifd::Exif, 0xa409),
    ("Sharpness", Ifd::Exif, 0xa40a),
    ("DeviceSettingDescription", Ifd::Exif, 0xa40b),
    ("SubjectDistanceRange", Ifd::Exif, 0xa40c),
    ("ImageUniqueID", Ifd::Exif, 0xa420),
    ("OwnerName", Ifd::Exif, 0xa430),
    ("SerialNumber", Ifd::Exif, 0xa431),
    ("LensMake", Ifd::Exif, 0xa433),
    ("LensModel", Ifd::Exif, 0xa434),
    ("LensSerialNumber", Ifd::Exif, 0xa435),
    // -- GPS sub-IFD -------------------------------------------------------
    ("GPSVersionID", Ifd::Gps, 0x0000),
    ("GPSLatitudeRef", Ifd::Gps, 0x0001),
    ("GPSLatitude", Ifd::Gps, 0x0002),
    ("GPSLongitudeRef", Ifd::Gps, 0x0003),
    ("GPSLongitude", Ifd::Gps, 0x0004),
    ("GPSAltitudeRef", Ifd::Gps, 0x0005),
    ("GPSAltitude", Ifd::Gps, 0x0006),
    ("GPSTimeStamp", Ifd::Gps, 0x0007),
    ("GPSSatellites", Ifd::Gps, 0x0008),
    ("GPSStatus", Ifd::Gps, 0x0009),
    ("GPSMeasureMode", Ifd::Gps, 0x000a),
    ("GPSDOP", Ifd::Gps, 0x000b),
    ("GPSSpeedRef", Ifd::Gps, 0x000c),
    ("GPSSpeed", Ifd::Gps, 0x000d),
    ("GPSTrackRef", Ifd::Gps, 0x000e),
    ("GPSTrack", Ifd::Gps, 0x000f),
    ("GPSImgDirectionRef", Ifd::Gps, 0x0010),
    ("GPSImgDirection", Ifd::Gps, 0x0011),
    ("GPSMapDatum", Ifd::Gps, 0x0012),
    ("GPSDestLatitudeRef", Ifd::Gps, 0x0013),
    ("GPSDestLatitude", Ifd::Gps, 0x0014),
    ("GPSDestLongitudeRef", Ifd::Gps, 0x0015),
    ("GPSDestLongitude", Ifd::Gps, 0x0016),
    ("GPSDestBearingRef", Ifd::Gps, 0x0017),
    ("GPSDestBearing", Ifd::Gps, 0x0018),
    ("GPSDestDistanceRef", Ifd::Gps, 0x0019),
    ("GPSDestDistance", Ifd::Gps, 0x001a),
    ("GPSProcessingMethod", Ifd::Gps, 0x001b),
    ("GPSAreaInformation", Ifd::Gps, 0x001c),
    ("GPSDateStamp", Ifd::Gps, 0x001d),
    ("GPSDifferential", Ifd::Gps, 0x001e),
    ("GPSHPositioningError", Ifd::Gps, 0x001f),
    // -- Interoperability sub-IFD -----------------------------------------
    ("InteroperabilityIndex", Ifd::Interop, 0x0001),
    ("InteroperabilityVersion", Ifd::Interop, 0x0002),
];

/// Resolves a single selector token from `--exif-only`/`--exif-except`.
///
/// Accepted forms:
/// - tag names from [`KNOWN_TAGS`] (case-insensitive), e.g. `DateTimeOriginal`
/// - IFD wildcards, e.g. `gps`, `ifd0`, `interoperability`
/// - numeric tag numbers (hex or decimal) paired with `Primary`, e.g. `0x8825`
///
/// Returns an error message for tokens that cannot be resolved.
pub fn parse_tag_selector(token: &str) -> Result<TagSelector, String> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return Err("empty tag selector".to_string());
    }

    if let Some(ifd) = Ifd::from_cli_name(trimmed) {
        return Ok(TagSelector::Ifd(ifd));
    }

    let lowered = trimmed.to_ascii_lowercase();
    if KNOWN_TAGS
        .iter()
        .any(|&(name, _, _)| name.eq_ignore_ascii_case(&lowered))
    {
        return Ok(TagSelector::Name(trimmed.to_string()));
    }

    // numeric fallback: bare tag number selects within the primary IFD
    let number = if let Some(hex) = lowered.strip_prefix("0x") {
        u16::from_str_radix(hex, 16)
    } else {
        trimmed.parse::<u16>()
    };
    match number {
        Ok(tag_num) => Ok(TagSelector::TagNum(tag_num, Ifd::Primary)),
        Err(_) => Err(format!(
            "unknown tag selector {trimmed:?}; see --exif-list-tags for recognized names, \
             use ifd wildcards (ifd0/ifd1/exif/gps/interop) or numeric tags (0x8825)"
        )),
    }
}

/// Parses a comma-separated selector list (`--exif-only`/`--exif-except`).
///
/// Returns the selectors, or an error message naming the first
/// unresolvable token.
pub fn parse_tag_list(specs: &[String]) -> Result<Vec<TagSelector>, String> {
    let mut selectors = Vec::new();
    for spec in specs {
        for token in spec.split(',') {
            selectors.push(parse_tag_selector(token)?);
        }
    }
    Ok(selectors)
}

/// Formats the recognized tag table plus IFD wildcards for `--exif-list-tags`.
///
/// The output is deterministic (golden-test friendly).
#[must_use]
pub fn format_recognized_tags() -> String {
    let mut out = String::from("Recognized EXIF tag names:\n");
    for &(name, ifd, tag_num) in KNOWN_TAGS {
        out.push_str(&format!("  {name} ({ifd}, 0x{tag_num:04x})\n"));
    }
    out.push_str("IFD wildcards:\n");
    for ifd in Ifd::ALL {
        let names = ifd
            .cli_names()
            .iter()
            .map(|name| (*name).to_string())
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("  {names}\n"));
    }
    out.push_str(
        "Numeric tags are accepted as 0x8825 or 34117 (resolved within ifd0).\n\
         Use --exif-list-tags to print this list.\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ifd_cli_names_resolve_case_insensitively() {
        assert_eq!(Ifd::from_cli_name("GPS"), Some(Ifd::Gps));
        assert_eq!(Ifd::from_cli_name("interop"), Some(Ifd::Interop));
        assert_eq!(Ifd::from_cli_name("INTEROPERABILITY"), Some(Ifd::Interop));
        assert_eq!(Ifd::from_cli_name("ifd0"), Some(Ifd::Primary));
        assert_eq!(Ifd::from_cli_name("nope"), None);
    }

    #[test]
    fn selectors_resolve_names_numbers_and_ifds() {
        assert_eq!(
            parse_tag_selector("datetimeoriginal").ok(),
            Some(TagSelector::Name("datetimeoriginal".to_string()))
        );
        assert_eq!(
            parse_tag_selector("0x8825").ok(),
            Some(TagSelector::TagNum(0x8825, Ifd::Primary))
        );
        assert_eq!(
            parse_tag_selector("34117").ok(),
            Some(TagSelector::TagNum(34117, Ifd::Primary))
        );
        assert_eq!(
            parse_tag_selector("gps").ok(),
            Some(TagSelector::Ifd(Ifd::Gps))
        );
        assert!(parse_tag_selector("NoSuchTag").is_err());
        assert!(parse_tag_selector("").is_err());
    }

    #[test]
    fn tag_lists_split_on_commas() {
        let specs = vec!["gps, Orientation".to_string(), "0x010f".to_string()];
        let selectors = parse_tag_list(&specs).expect("valid list");
        assert_eq!(
            selectors,
            vec![
                TagSelector::Ifd(Ifd::Gps),
                TagSelector::Name("Orientation".to_string()),
                TagSelector::TagNum(0x010f, Ifd::Primary),
            ]
        );
    }

    #[test]
    fn recognized_tag_output_is_stable() {
        let first = format_recognized_tags();
        let second = format_recognized_tags();
        assert_eq!(first, second, "output must be deterministic");
        assert!(first.contains("DateTimeOriginal"));
        assert!(first.contains("Orientation"));
        assert!(first.contains("GPSInfo"));
        assert!(first.contains("IFD wildcards:"));
        assert!(first.starts_with("Recognized EXIF tag names:"));
    }

    #[test]
    fn policy_defaults_to_strip() {
        assert_eq!(ExifPolicy::default(), ExifPolicy::Strip);
        assert!(!ExifPolicy::Strip.wants_metadata());
        assert!(ExifPolicy::Keep.wants_metadata());
        assert_eq!(
            ExifPolicy::FilterExcept(vec![TagSelector::Ifd(Ifd::Gps)]).describe(),
            "except gps"
        );
    }
}
