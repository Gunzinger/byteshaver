//! Metadata embedding helpers for the target encoders.
//!
//! Each helper takes the **raw TIFF payload** (the canonical internal
//! convention, see [`crate::metadata`]) and produces whatever the target
//! container needs.

/// JPEG APP1 segment header preceding the TIFF stream.
pub const EXIF_APP1_HEADER: &[u8] = b"Exif\0\0";

/// Maximum payload of a single JPEG marker segment (65,533 bytes).
const JPEG_MAX_MARKER_PAYLOAD: usize = 65533;

/// Builds the JPEG APP1 marker payload: `b"Exif\0\0"` + TIFF payload.
///
/// Returns `None` if the payload does not fit into a single APP1 segment;
/// callers skip embedding with a warning in that case (mozjpeg's
/// `write_marker` cannot split markers).
#[must_use]
pub fn exif_app1_marker(tiff_payload: &[u8]) -> Option<Vec<u8>> {
    let total = EXIF_APP1_HEADER.len() + tiff_payload.len();
    if total > JPEG_MAX_MARKER_PAYLOAD {
        return None;
    }
    let mut marker = Vec::with_capacity(total);
    marker.extend_from_slice(EXIF_APP1_HEADER);
    marker.extend_from_slice(tiff_payload);
    Some(marker)
}

/// Embeds a raw TIFF payload into an encoded WebP container via the RIFF
/// muxer.
///
/// Returns `Some(muxed_bytes)` on success, `None` if the container could
/// not be rebuilt (callers keep the un-muxed bytes and warn).
#[must_use]
pub fn embed_webp(webp: &[u8], tiff_payload: &[u8]) -> Option<Vec<u8>> {
    crate::metadata::riff::mux_exif(webp, tiff_payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app1_marker_layout_and_size_guard() {
        let payload = b"II*\0\x08\0\0\0";
        let marker = exif_app1_marker(payload).expect("small payload");
        assert_eq!(&marker[..6], EXIF_APP1_HEADER);
        assert_eq!(&marker[6..], payload);

        let huge = vec![0u8; JPEG_MAX_MARKER_PAYLOAD];
        assert!(exif_app1_marker(&huge).is_none(), "payload must not fit");
    }

    #[test]
    fn webp_embedding_delegates_to_riff_muxer() {
        let vp8l_header: u32 = 3 | 3 << 14; // 4x4, no alpha
        let mut payload = vec![0x2f];
        payload.extend_from_slice(&vp8l_header.to_le_bytes());
        payload.push(0);
        let webp = {
            let mut out = b"RIFF".to_vec();
            let body = 4u32 + 8 + payload.len() as u32;
            out.extend_from_slice(&body.to_le_bytes());
            out.extend_from_slice(b"WEBPVP8L");
            out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            out.extend_from_slice(&payload);
            out
        };
        let muxed = embed_webp(&webp, b"II*\0\x08\0\0\0\0\0").expect("embedded");
        assert!(crate::metadata::riff::extract_exif_payload(&muxed).is_some());
    }
}
