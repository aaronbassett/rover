//! Header-only image dimension parser. Reads the first few bytes of common
//! image formats (PNG, JPEG, WebP, GIF) to extract `(width, height)` without
//! decoding the image data. Used by the always-on caption-filter pipeline to
//! gate captioning of icons/thumbnails without pulling in the full `image`
//! crate's decoders (which alone would push the default binary over the
//! 25 MiB budget — PRD §15).
//!
//! For formats not in this set, or for malformed headers, returns `None` and
//! the caller treats dimensions as indeterminate (gate passes; size gate
//! still applies).

pub fn peek_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 8 {
        return None;
    }
    // PNG
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        return parse_png(bytes);
    }
    // JPEG
    if bytes.starts_with(&[0xFF, 0xD8]) {
        return parse_jpeg(bytes);
    }
    // WebP
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return parse_webp(bytes);
    }
    // GIF
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return parse_gif(bytes);
    }
    None
}

fn parse_png(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 {
        return None;
    }
    if &bytes[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((w, h))
}

fn parse_jpeg(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2usize;
    while i + 9 < bytes.len() {
        if bytes[i] != 0xFF {
            return None;
        }
        let marker = bytes[i + 1];
        // SOF markers: 0xC0..=0xCF except 0xC4 (DHT), 0xC8 (JPG), 0xCC (DAC).
        if (0xC0..=0xCF).contains(&marker) && marker != 0xC4 && marker != 0xC8 && marker != 0xCC {
            // SOF: marker(2) + length(2) + precision(1) → height(2 BE), width(2 BE).
            let h = u16::from_be_bytes(bytes[i + 5..i + 7].try_into().ok()?);
            let w = u16::from_be_bytes(bytes[i + 7..i + 9].try_into().ok()?);
            return Some((w as u32, h as u32));
        }
        // Skip this segment: marker(2) + length(2 BE) where length includes the 2 length bytes.
        let seg_len = u16::from_be_bytes(bytes[i + 2..i + 4].try_into().ok()?) as usize;
        i = i.checked_add(2)?.checked_add(seg_len)?;
    }
    None
}

fn parse_webp(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 30 {
        return None;
    }
    // Chunk header at bytes 12-15: "VP8 ", "VP8L", or "VP8X".
    match &bytes[12..16] {
        b"VP8X" => {
            // Extended: width-1 (3 LE) at 24-26, height-1 (3 LE) at 27-29.
            let w_minus_1 = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], 0]);
            let h_minus_1 = u32::from_le_bytes([bytes[27], bytes[28], bytes[29], 0]);
            Some((w_minus_1 + 1, h_minus_1 + 1))
        }
        b"VP8L" => {
            // Lossless: signature byte 0x2F at chunk_data byte 0 (byte 20), then
            // 14-bit width-1 and 14-bit height-1 packed into bytes 21-24.
            if bytes.len() < 25 {
                return None;
            }
            let b1 = bytes[21] as u32;
            let b2 = bytes[22] as u32;
            let b3 = bytes[23] as u32;
            let b4 = bytes[24] as u32;
            let w = (b1 | ((b2 & 0x3F) << 8)) + 1;
            let h = (((b2 >> 6) & 0x03) | (b3 << 2) | ((b4 & 0x0F) << 10)) + 1;
            Some((w, h))
        }
        b"VP8 " => {
            // Lossy: chunk_data starts at byte 20. After the 3-byte frame tag
            // comes the start-code magic 0x9D 0x01 0x2A at bytes 23-25, then
            // width (14 bits) at 26-27 LE and height (14 bits) at 28-29 LE.
            if bytes.len() < 30 {
                return None;
            }
            if bytes[23..26] != [0x9D, 0x01, 0x2A] {
                return None;
            }
            let w = u16::from_le_bytes(bytes[26..28].try_into().ok()?) & 0x3FFF;
            let h = u16::from_le_bytes(bytes[28..30].try_into().ok()?) & 0x3FFF;
            Some((w as u32, h as u32))
        }
        _ => None,
    }
}

fn parse_gif(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 10 {
        return None;
    }
    let w = u16::from_le_bytes(bytes[6..8].try_into().ok()?);
    let h = u16::from_le_bytes(bytes[8..10].try_into().ok()?);
    Some((w as u32, h as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_1x1() {
        let png: [u8; 24] = [
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, b'I', b'H',
            b'D', b'R', 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
        ];
        assert_eq!(peek_dimensions(&png), Some((1, 1)));
    }

    #[test]
    fn png_200x200() {
        let png: [u8; 24] = [
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, b'I', b'H',
            b'D', b'R', 0x00, 0x00, 0x00, 0xC8, 0x00, 0x00, 0x00, 0xC8,
        ];
        assert_eq!(peek_dimensions(&png), Some((200, 200)));
    }

    #[test]
    fn jpeg_with_sof0() {
        // FF D8 (SOI) + FF E0 ... (APP0) + FF C0 (SOF0) + len + precision + height + width
        let jpeg: [u8; 20] = [
            0xFF, 0xD8, // SOI
            0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00, // APP0 with length=4 (incl length bytes)
            0xFF, 0xC0, 0x00, 0x11, 0x08, // SOF0 marker + length + precision
            0x00, 0x80, 0x00, 0xC0, // height=128, width=192
            0x03, 0x01, 0x22, // components (truncated)
        ];
        assert_eq!(peek_dimensions(&jpeg), Some((192, 128)));
    }

    #[test]
    fn gif_400x300() {
        let gif: [u8; 10] = [
            b'G', b'I', b'F', b'8', b'9', b'a', 0x90, 0x01, // 400 LE
            0x2C, 0x01, // 300 LE
        ];
        assert_eq!(peek_dimensions(&gif), Some((400, 300)));
    }

    #[test]
    fn webp_vp8x_500x500() {
        let mut webp = vec![0u8; 30];
        webp[0..4].copy_from_slice(b"RIFF");
        webp[8..12].copy_from_slice(b"WEBP");
        webp[12..16].copy_from_slice(b"VP8X");
        // width-1 = 499 = 0x01F3 (3 LE: F3 01 00)
        webp[24] = 0xF3;
        webp[25] = 0x01;
        webp[26] = 0x00;
        // height-1 = 499
        webp[27] = 0xF3;
        webp[28] = 0x01;
        webp[29] = 0x00;
        assert_eq!(peek_dimensions(&webp), Some((500, 500)));
    }

    #[test]
    fn unknown_format_returns_none() {
        assert_eq!(peek_dimensions(b"not an image"), None);
        assert_eq!(peek_dimensions(&[]), None);
    }

    #[test]
    fn truncated_png_header_returns_none() {
        // Magic only, no IHDR.
        let bytes: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        assert_eq!(peek_dimensions(&bytes), None);
    }
}
