//! Charset detection pipeline (PRD §5.1).
//!
//! Order:
//!   1. BOM (`encoding_rs::Encoding::for_bom`)
//!   2. HTTP `Content-Type` charset parameter (`Encoding::for_label`)
//!   3. ASCII-decode first 1024 bytes, regex-scan for `<meta charset=...>` /
//!      `<meta http-equiv="Content-Type" content="...; charset=...">`
//!   4. `chardetng::EncodingDetector::guess(None, true)`
//!   5. UTF-8 with replacement.
//!
//! `readabilityrs` accepts `&str`, so we always re-encode the final output to
//! UTF-8 here.

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use encoding_rs::{Encoding, UTF_8};
use regex::Regex;
use std::sync::LazyLock;

/// What sniffing approach picked the encoding. Useful for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectionSource {
    Bom,
    HttpHeader,
    MetaTag,
    Chardetng,
    Fallback,
}

#[derive(Debug, Clone, Copy)]
pub struct Detected {
    pub encoding: &'static Encoding,
    pub source: DetectionSource,
}

/// Detect the source encoding for an HTTP response body.
pub fn detect_encoding(content_type: Option<&str>, bytes: &[u8]) -> Detected {
    // 1. BOM
    if let Some((enc, _bom_len)) = Encoding::for_bom(bytes) {
        return Detected { encoding: enc, source: DetectionSource::Bom };
    }

    // 2. HTTP Content-Type charset
    if let Some(ct) = content_type {
        if let Some(label) = parse_charset_param(ct) {
            if let Some(enc) = Encoding::for_label(label.as_bytes()) {
                return Detected { encoding: enc, source: DetectionSource::HttpHeader };
            }
        }
    }

    // 3. <meta> sniff in first 1024 bytes
    if let Some(enc) = sniff_meta_charset(bytes) {
        return Detected { encoding: enc, source: DetectionSource::MetaTag };
    }

    // 4. chardetng
    let mut det = EncodingDetector::new(Iso2022JpDetection::Deny);
    det.feed(bytes, true);
    let enc = det.guess(None, Utf8Detection::Allow);
    if enc != UTF_8 || looks_like_utf8(bytes) {
        return Detected { encoding: enc, source: DetectionSource::Chardetng };
    }

    // 5. Fallback
    Detected { encoding: UTF_8, source: DetectionSource::Fallback }
}

/// Decode `bytes` to UTF-8 using the result of [`detect_encoding`].
///
/// Returns the decoded string and the detection result so callers can log
/// HTTP-vs-detected mismatches.
pub fn decode_to_utf8(content_type: Option<&str>, bytes: &[u8]) -> (String, Detected) {
    let detected = detect_encoding(content_type, bytes);
    let (cow, _enc_used, _had_errors) = detected.encoding.decode(bytes);
    (cow.into_owned(), detected)
}

/// Extract the charset parameter from a `Content-Type` header value.
fn parse_charset_param(header: &str) -> Option<String> {
    for part in header.split(';').map(str::trim) {
        if let Some(rest) = part.strip_prefix_ignore_case("charset=") {
            return Some(strip_quotes(rest).to_string());
        }
    }
    None
}

trait StripPrefixIgnoreCase {
    fn strip_prefix_ignore_case<'a>(&'a self, prefix: &str) -> Option<&'a str>;
}

impl StripPrefixIgnoreCase for str {
    fn strip_prefix_ignore_case<'a>(&'a self, prefix: &str) -> Option<&'a str> {
        if self.len() < prefix.len() { return None; }
        let head = &self[..prefix.len()];
        if head.eq_ignore_ascii_case(prefix) {
            Some(&self[prefix.len()..])
        } else {
            None
        }
    }
}

fn strip_quotes(s: &str) -> &str {
    let s = s.trim();
    if s.len() >= 2 && (s.starts_with('"') && s.ends_with('"')) {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

/// Look for `<meta charset>` or `<meta http-equiv="Content-Type" ...>` in the
/// first 1024 bytes, ASCII-decoded.
fn sniff_meta_charset(bytes: &[u8]) -> Option<&'static Encoding> {
    static META_CHARSET: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?xi)
            <meta \s [^>]*?
            (?:
                charset \s* = \s* ["']? ([A-Za-z0-9_:.\-]+)
              | http-equiv \s* = \s* ["']? content-type ["']? \s [^>]*?
                content \s* = \s* ["']? [^"'>]*? charset \s* = \s* ([A-Za-z0-9_:.\-]+)
            )
        "#).unwrap()
    });

    let head_len = bytes.len().min(1024);
    let head: String = bytes[..head_len].iter()
        .map(|&b| if b.is_ascii() { b as char } else { ' ' })
        .collect();
    let caps = META_CHARSET.captures(&head)?;
    let label = caps.get(1).or_else(|| caps.get(2))?.as_str();
    Encoding::for_label(label.as_bytes())
}

/// Quick UTF-8 plausibility check used to disambiguate the chardetng default.
fn looks_like_utf8(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_utf8_bom() {
        let bytes = b"\xEF\xBB\xBFhello";
        let det = detect_encoding(None, bytes);
        assert_eq!(det.source, DetectionSource::Bom);
        assert_eq!(det.encoding, UTF_8);
    }

    #[test]
    fn detects_utf16le_bom() {
        let bytes = b"\xFF\xFEh\x00i\x00";
        let det = detect_encoding(None, bytes);
        assert_eq!(det.source, DetectionSource::Bom);
    }

    #[test]
    fn detects_from_http_header() {
        let bytes = b"<html><body>caf\xE9</body></html>";
        let det = detect_encoding(Some("text/html; charset=ISO-8859-1"), bytes);
        assert_eq!(det.source, DetectionSource::HttpHeader);
        assert_eq!(det.encoding.name(), "windows-1252"); // encoding_rs maps Latin-1 -> windows-1252
    }

    #[test]
    fn detects_from_http_header_with_quotes() {
        let bytes = b"hello";
        let det = detect_encoding(Some(r#"text/html; charset="utf-8""#), bytes);
        assert_eq!(det.source, DetectionSource::HttpHeader);
        assert_eq!(det.encoding, UTF_8);
    }

    #[test]
    fn detects_from_meta_charset() {
        let html = br#"<!doctype html><html><head><meta charset="Shift_JIS"></head>"#;
        let det = detect_encoding(None, html);
        assert_eq!(det.source, DetectionSource::MetaTag);
        assert_eq!(det.encoding.name(), "Shift_JIS");
    }

    #[test]
    fn detects_from_meta_http_equiv() {
        let html = br#"<html><head><meta http-equiv="Content-Type" content="text/html; charset=EUC-KR"></head>"#;
        let det = detect_encoding(None, html);
        assert_eq!(det.source, DetectionSource::MetaTag);
        assert_eq!(det.encoding.name(), "EUC-KR");
    }

    #[test]
    fn falls_back_to_chardetng_for_plain_utf8() {
        let bytes = "héllo wörld".as_bytes();
        let det = detect_encoding(None, bytes);
        assert!(matches!(det.source, DetectionSource::Chardetng | DetectionSource::Fallback));
        assert_eq!(det.encoding, UTF_8);
    }

    #[test]
    fn header_overrides_meta() {
        // Header says UTF-8, meta says Shift_JIS — header wins.
        let html = br#"<html><head><meta charset="Shift_JIS"></head>"#;
        let det = detect_encoding(Some("text/html; charset=utf-8"), html);
        assert_eq!(det.source, DetectionSource::HttpHeader);
    }

    #[test]
    fn invalid_label_in_header_falls_through() {
        let html = br#"<html><head><meta charset="utf-8"></head>"#;
        let det = detect_encoding(Some("text/html; charset=not-a-real-charset"), html);
        assert_eq!(det.source, DetectionSource::MetaTag);
    }

    #[test]
    fn decode_round_trips_utf8() {
        let (out, det) = decode_to_utf8(Some("text/html; charset=utf-8"), "héllo".as_bytes());
        assert_eq!(out, "héllo");
        assert_eq!(det.encoding, UTF_8);
    }

    #[test]
    fn decode_handles_latin1() {
        // 0xE9 is é in ISO-8859-1.
        let (out, _det) = decode_to_utf8(Some("text/html; charset=ISO-8859-1"), &[b'h', 0xE9, b'l', b'l', b'o']);
        assert_eq!(out, "héllo");
    }
}
