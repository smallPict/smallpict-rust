//! Image type from the first bytes, for callers who pass data without a `mime_type`.

/// The media type the magic bytes identify, or `None` when they match no supported format.
pub(crate) fn detect_mime(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\x89PNG") {
        Some("image/png")
    } else if data.starts_with(b"\xFF\xD8\xFF") {
        Some("image/jpeg")
    } else if data.starts_with(b"GIF8") {
        Some("image/gif")
    } else if data.len() >= 12 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP" {
        Some("image/webp")
    } else if data.len() >= 12 && &data[4..12] == b"ftypavif" {
        Some("image/avif")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::detect_mime;

    #[test]
    fn short_and_unknown_inputs_are_none() {
        assert_eq!(detect_mime(b""), None);
        assert_eq!(detect_mime(b"RIFF"), None);
        assert_eq!(detect_mime(b"not an image"), None);
    }
}
