//! Minimal, pure safe-Rust WebP RIFF container parser and muxer.
//!
//! Used for two things:
//! - decode-side fallback: extracting the `EXIF` chunk from a WebP
//!   container when the decoder exposes no EXIF payload,
//! - write-side: embedding an `EXIF` chunk into an encoded WebP container
//!   (rebuilding the chunk list and the `VP8X` flags per the WebP container
//!   specification: `VP8X`, `ICCP`, image data (`ANIM`/`ANMF`/`ALPH`/
//!   `VP8 `/`VP8L`), then `EXIF`, `XMP `, then unknown chunks).
//!
//! No `unsafe`; all sizes are checked before slicing.

/// FourCC of the top-level RIFF header.
const FCC_RIFF: [u8; 4] = *b"RIFF";
/// FourCC of the WebP form type.
const FCC_WEBP: [u8; 4] = *b"WEBP";
/// FourCC of the extended-format feature header.
const FCC_VP8X: [u8; 4] = *b"VP8X";
/// FourCC of the EXIF metadata chunk.
const FCC_EXIF: [u8; 4] = *b"EXIF";
/// JPEG APP1-style Exif header (`"Exif\0\0"`) that may prefix the TIFF
/// stream inside a WebP `EXIF` chunk.
const EXIF_HEADER: [u8; 6] = *b"Exif\0\0";

/// VP8X flag bit for "file contains an ICC profile" (byte 0, bit 5).
const VP8X_FLAG_ICC: u8 = 1 << 5;
/// VP8X flag bit for "file contains alpha transparency" (byte 0, bit 4).
const VP8X_FLAG_ALPHA: u8 = 1 << 4;
/// VP8X flag bit for "file contains Exif metadata" (byte 0, bit 3).
const VP8X_FLAG_EXIF: u8 = 1 << 3;
/// VP8X flag bit for "file contains XMP metadata" (byte 0, bit 2).
///
/// Declared for completeness of the flag table; XMP chunks are preserved
/// as-is by the muxer and never written by this crate, so the bit is only
/// read-through when (re)building headers is extended later.
#[allow(dead_code)]
const VP8X_FLAG_XMP: u8 = 1 << 2;
/// VP8X flag bit for "file is animated" (byte 0, bit 1).
const VP8X_FLAG_ANIMATION: u8 = 1 << 1;

/// A single top-level RIFF chunk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    /// Chunk FourCC.
    pub id: [u8; 4],
    /// Chunk payload (padding byte already removed).
    pub payload: Vec<u8>,
}

impl Chunk {
    /// Builds a chunk from a FourCC and a payload.
    fn new(id: [u8; 4], payload: Vec<u8>) -> Self {
        Chunk { id, payload }
    }
}

/// Parses the top-level chunks of a WebP container.
///
/// Returns `None` if the data is not a recognizable WebP RIFF container
/// (missing RIFF/WEBP signature or truncated chunk headers).
#[must_use]
pub fn parse_webp(data: &[u8]) -> Option<Vec<Chunk>> {
    if data.len() < 12 || data[0..4] != FCC_RIFF || data[8..12] != FCC_WEBP {
        return None;
    }
    // declared size (from offset 8) clamped to the actually available bytes;
    // readers are expected to be tolerant here
    let declared = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
    let available = data.len() - 8;
    let body_len = if (4..=available).contains(&declared) {
        declared - 4
    } else {
        available - 4
    };

    let mut chunks = Vec::new();
    let mut offset = 12;
    let end = 12 + body_len;
    while offset + 8 <= end.min(data.len()) {
        let id: [u8; 4] = data[offset..offset + 4].try_into().ok()?;
        let size = u32::from_le_bytes(data[offset + 4..offset + 8].try_into().ok()?) as usize;
        offset += 8;
        let payload_end = offset.checked_add(size)?;
        if payload_end > data.len() {
            // truncated final chunk: keep what is there? No — malformed.
            return None;
        }
        chunks.push(Chunk::new(id, data[offset..payload_end].to_vec()));
        offset = payload_end;
        // odd-sized chunks are followed by one padding byte
        if size % 2 == 1 {
            offset += 1;
        }
    }
    Some(chunks)
}

/// Serializes chunks back into a WebP container (RIFF header + padding).
fn write_webp(chunks: &[Chunk]) -> Vec<u8> {
    let body_size: usize = chunks
        .iter()
        .map(|chunk| 8 + chunk.payload.len() + (chunk.payload.len() % 2))
        .sum();
    let mut out = Vec::with_capacity(12 + body_size);
    out.extend_from_slice(&FCC_RIFF);
    // RIFF size: everything after the size field (WEBP fourcc + chunks)
    out.extend_from_slice(
        &u32::try_from(4 + body_size)
            .unwrap_or(u32::MAX)
            .to_le_bytes(),
    );
    out.extend_from_slice(&FCC_WEBP);
    for chunk in chunks {
        out.extend_from_slice(&chunk.id);
        out.extend_from_slice(
            &u32::try_from(chunk.payload.len())
                .unwrap_or(u32::MAX)
                .to_le_bytes(),
        );
        out.extend_from_slice(&chunk.payload);
        if chunk.payload.len() % 2 == 1 {
            out.push(0);
        }
    }
    out
}

/// Categorizes a chunk for the spec-compliant ordering.
fn order_rank(id: &[u8; 4]) -> u8 {
    match id {
        _ if id == &FCC_VP8X => 0,
        b"ICCP" => 1,
        b"ANIM" | b"ANMF" | b"ALPH" | b"VP8 " | b"VP8L" => 2,
        _ if id == &FCC_EXIF => 4,
        b"XMP " => 5,
        _ => 6, // unknown chunks stay at the end, in their original order
    }
}

/// Extracts the EXIF payload of a WebP container as **raw TIFF bytes**
/// (the `b"Exif\0\0"` chunk prefix, if present, is stripped).
#[must_use]
pub fn extract_exif_payload(webp: &[u8]) -> Option<Vec<u8>> {
    let chunks = parse_webp(webp)?;
    let chunk = chunks.iter().find(|chunk| chunk.id == FCC_EXIF)?;
    let payload = strip_exif_header(&chunk.payload);
    if payload.is_empty() {
        None
    } else {
        Some(payload.to_vec())
    }
}

/// Strips the `b"Exif\0\0"` header from an EXIF chunk body, if present.
#[must_use]
pub fn strip_exif_header(body: &[u8]) -> &[u8] {
    if body.len() > EXIF_HEADER.len() && body[..EXIF_HEADER.len()] == EXIF_HEADER {
        &body[EXIF_HEADER.len()..]
    } else {
        body
    }
}

/// Embeds `tiff_payload` (raw TIFF bytes) as the `EXIF` chunk of a WebP
/// container, rebuilding the chunk list per the WebP container spec and
/// creating or updating the `VP8X` header (EXIF flag).
///
/// An existing `EXIF` chunk is replaced. Returns `None` if the input is not
/// a valid WebP container or has no VP8/VP8L image data to derive a missing
/// `VP8X` header from.
#[must_use]
pub fn mux_exif(webp: &[u8], tiff_payload: &[u8]) -> Option<Vec<u8>> {
    if tiff_payload.is_empty() {
        return None;
    }
    let mut chunks = parse_webp(webp)?;
    if !chunks
        .iter()
        .any(|chunk| chunk.id == *b"VP8 " || chunk.id == *b"VP8L" || chunk.id == *b"ANMF")
    {
        return None;
    }

    // replace an existing EXIF chunk
    chunks.retain(|chunk| chunk.id != FCC_EXIF);

    match chunks.iter_mut().find(|chunk| chunk.id == FCC_VP8X) {
        Some(vp8x) => {
            if let Some(flags) = vp8x.payload.first_mut() {
                *flags |= VP8X_FLAG_EXIF;
            }
        }
        None => {
            let vp8x = build_vp8x(&chunks)?;
            // VP8X always comes first per spec
            chunks.insert(0, Chunk::new(FCC_VP8X, vp8x));
        }
    }

    // stable sort into spec order: VP8X, ICCP, image data, EXIF, XMP, unknown
    let mut ranked: Vec<(u8, Chunk)> = chunks
        .into_iter()
        .map(|chunk| {
            let rank = order_rank(&chunk.id);
            (rank, chunk)
        })
        .collect();
    let exif = Chunk::new(FCC_EXIF, embed_body(tiff_payload));
    ranked.push((order_rank(&FCC_EXIF), exif));
    ranked.sort_by_key(|(rank, _)| *rank); // sort_by_key is stable

    let ordered: Vec<Chunk> = ranked.into_iter().map(|(_, chunk)| chunk).collect();
    Some(write_webp(&ordered))
}

/// Wraps the raw TIFF payload into the WebP EXIF chunk body
/// (`b"Exif\0\0"` + TIFF stream, per the WebP container spec metadata
/// convention).
fn embed_body(tiff_payload: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(EXIF_HEADER.len() + tiff_payload.len());
    body.extend_from_slice(&EXIF_HEADER);
    body.extend_from_slice(tiff_payload);
    body
}

/// Builds a VP8X payload for a container that lacks one (simple format
/// lossy/lossless outputs), deriving canvas size and alpha flag from the
/// VP8/VP8L bitstream.
fn build_vp8x(chunks: &[Chunk]) -> Option<Vec<u8>> {
    for chunk in chunks {
        match &chunk.id {
            b"VP8L" => {
                if let Some(((width, height), alpha)) = parse_vp8l_header(&chunk.payload) {
                    return Some(vp8x_payload(
                        width,
                        height,
                        alpha,
                        has_icc(chunks),
                        has_anim(chunks),
                        true,
                    ));
                }
            }
            b"VP8 " => {
                if let Some(((width, height), _)) = parse_vp8_header(&chunk.payload) {
                    // lossy alpha lives in a separate ALPH chunk, which
                    // implies an extended container (VP8X already present);
                    // bare VP8 key frames have no alpha channel
                    let alpha = false;
                    return Some(vp8x_payload(
                        width,
                        height,
                        alpha,
                        has_icc(chunks),
                        has_anim(chunks),
                        true,
                    ));
                }
            }
            _ => {}
        }
    }
    None
}

fn has_icc(chunks: &[Chunk]) -> bool {
    chunks.iter().any(|chunk| chunk.id == *b"ICCP")
}

fn has_anim(chunks: &[Chunk]) -> bool {
    chunks.iter().any(|chunk| chunk.id == *b"ANIM")
}

/// Assembles the 10-byte VP8X payload (flags + canvas size minus one).
///
/// The EXIF flag is passed explicitly: `true` when the muxer is about to
/// embed an EXIF chunk, `false` for hand-built test containers.
fn vp8x_payload(
    width: u32,
    height: u32,
    alpha: bool,
    icc: bool,
    anim: bool,
    exif: bool,
) -> Vec<u8> {
    let mut flags = 0u8;
    if icc {
        flags |= VP8X_FLAG_ICC;
    }
    if alpha {
        flags |= VP8X_FLAG_ALPHA;
    }
    if anim {
        flags |= VP8X_FLAG_ANIMATION;
    }
    if exif {
        flags |= VP8X_FLAG_EXIF;
    }
    let mut payload = Vec::with_capacity(10);
    payload.push(flags);
    payload.extend_from_slice(&[0, 0, 0]); // reserved
    payload.extend_from_slice(&(width - 1).to_le_bytes()[..3]);
    payload.extend_from_slice(&(height - 1).to_le_bytes()[..3]);
    payload
}

/// Parses canvas dimensions and the alpha flag from a VP8L bitstream header.
///
/// Returns `(None, ..)`-style: `None` if the header is malformed.
fn parse_vp8l_header(payload: &[u8]) -> Option<((u32, u32), bool)> {
    if payload.len() < 5 || payload[0] != 0x2f {
        return None;
    }
    let header = u32::from_le_bytes(payload[1..5].try_into().ok()?);
    let width = (header & 0x3fff) + 1;
    let height = ((header >> 14) & 0x3fff) + 1;
    let alpha = (header >> 28) & 1 == 1;
    Some(((width, height), alpha))
}

/// Parses canvas dimensions from a VP8 key frame header.
fn parse_vp8_header(payload: &[u8]) -> Option<((u32, u32), bool)> {
    if payload.len() < 10 {
        return None;
    }
    let frame_tag =
        u32::from(payload[0]) | u32::from(payload[1]) << 8 | u32::from(payload[2]) << 16;
    let key_frame = frame_tag & 1 == 0;
    if !key_frame || payload[3..6] != [0x9d, 0x01, 0x2a] {
        return None;
    }
    let width = u32::from(payload[6]) | u32::from(payload[7]) << 8;
    let height = u32::from(payload[8]) | u32::from(payload[9]) << 8;
    let width = (width & 0x3fff) + (((width & 0xc000) >> 14) << 14);
    let height = (height & 0x3fff) + (((height & 0xc000) >> 14) << 14);
    Some(((width, height), true))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_vp8l(width: u32, height: u32, alpha: bool) -> Chunk {
        let header: u32 = (width - 1) | (height - 1) << 14 | u32::from(alpha) << 28;
        let mut payload = vec![0x2f];
        payload.extend_from_slice(&header.to_le_bytes());
        payload.extend_from_slice(&[0x00]); // dummy bitstream byte
        Chunk::new(*b"VP8L", payload)
    }

    #[test]
    fn parse_rejects_non_webp_and_accepts_simple_container() {
        assert!(parse_webp(b"not a webp").is_none());
        let webp = write_webp(&[minimal_vp8l(4, 2, false)]);
        let chunks = parse_webp(&webp).expect("valid container");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].id, *b"VP8L");
    }

    #[test]
    fn extract_finds_exif_and_strips_marker_prefix() {
        let tiff = b"II*\0\x08\0\0\0\0\0".to_vec();
        let mut body = EXIF_HEADER.to_vec();
        body.extend_from_slice(&tiff);
        let webp = write_webp(&[
            Chunk::new(FCC_VP8X, vp8x_payload(4, 2, false, false, false, false)),
            minimal_vp8l(4, 2, false),
            Chunk::new(FCC_EXIF, body),
        ]);
        assert_eq!(
            extract_exif_payload(&webp).as_deref(),
            Some(tiff.as_slice())
        );
    }

    #[test]
    fn extract_handles_prefixless_bodies_and_missing_chunks() {
        let tiff = b"II*\0\x08\0\0\0\0\0";
        let webp = write_webp(&[
            Chunk::new(FCC_VP8X, vp8x_payload(4, 2, false, false, false, false)),
            Chunk::new(FCC_EXIF, tiff.to_vec()),
        ]);
        assert_eq!(
            extract_exif_payload(&webp).as_deref(),
            Some(tiff.as_slice())
        );

        let plain = write_webp(&[minimal_vp8l(4, 2, false)]);
        assert!(extract_exif_payload(&plain).is_none());
        assert!(extract_exif_payload(b"RIFF").is_none());
    }

    #[test]
    fn mux_adds_vp8x_with_exif_flag_for_simple_containers() {
        let webp = write_webp(&[minimal_vp8l(4, 2, true)]);
        let tiff = b"II*\0\x08\0\0\0\0\0";
        let muxed = mux_exif(&webp, tiff).expect("muxed");
        let chunks = parse_webp(&muxed).expect("parses");
        assert_eq!(chunks.len(), 3, "VP8X + VP8L + EXIF");
        assert_eq!(chunks[0].id, *b"VP8X");
        // EXIF flag set, alpha flag preserved (VP8L header reported alpha)
        assert_eq!(chunks[0].payload[0] & VP8X_FLAG_EXIF, VP8X_FLAG_EXIF);
        assert_eq!(chunks[0].payload[0] & VP8X_FLAG_ALPHA, VP8X_FLAG_ALPHA);
        // canvas size
        let w = u32::from(chunks[0].payload[4])
            | u32::from(chunks[0].payload[5]) << 8
            | u32::from(chunks[0].payload[6]) << 16;
        assert_eq!(w + 1, 4);
        // spec order: VP8X first, image data second, EXIF last
        assert_eq!(chunks[1].id, *b"VP8L");
        assert_eq!(chunks[2].id, FCC_EXIF);
    }

    #[test]
    fn mux_replaces_existing_exif_and_keeps_order() {
        let tiff_a = b"II*\0\x08\0\0\0\0\0";
        let container = mux_exif(&write_webp(&[minimal_vp8l(4, 2, false)]), tiff_a).expect("muxed");

        let tiff_b = b"MM\0*\0\0\0\x10\0\0".to_vec();
        let remuxed = mux_exif(&container, &tiff_b).expect("remuxed");
        let chunks = parse_webp(&remuxed).expect("parses");
        let exif_chunks = chunks.iter().filter(|chunk| chunk.id == FCC_EXIF).count();
        assert_eq!(exif_chunks, 1, "existing EXIF chunk must be replaced");
        let stored = chunks
            .iter()
            .find(|chunk| chunk.id == FCC_EXIF)
            .expect("exif");
        assert_eq!(strip_exif_header(&stored.payload), tiff_b.as_slice());
    }

    #[test]
    fn mux_rejects_malformed_input_and_empty_payloads() {
        assert!(mux_exif(b"garbage data!", b"II*\0").is_none());
        let webp = write_webp(&[minimal_vp8l(4, 2, false)]);
        assert!(mux_exif(&webp, &[]).is_none());
    }

    #[test]
    fn mux_preserves_odd_sized_unknown_chunks() {
        let unknown = Chunk::new(*b"XYZW", vec![0xaa]);
        let webp = write_webp(&[minimal_vp8l(4, 2, false), unknown]);
        let muxed = mux_exif(&webp, b"II*\0\x08\0\0\0\0\0").expect("muxed");
        let chunks = parse_webp(&muxed).expect("parses");
        let xyzw = chunks
            .iter()
            .find(|chunk| chunk.id == *b"XYZW")
            .expect("kept");
        assert_eq!(xyzw.payload, vec![0xaa]);
        // order: VP8X(0), VP8L(2), EXIF(4), unknown(6)
        let ids: Vec<[u8; 4]> = chunks.iter().map(|chunk| chunk.id).collect();
        assert_eq!(ids, vec![*b"VP8X", *b"VP8L", FCC_EXIF, *b"XYZW"]);
    }
}
