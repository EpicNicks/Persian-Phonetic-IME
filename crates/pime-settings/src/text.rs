//! Writing `config.toml` back in the encoding it was read in.
//!
//! Decoding lives in `pime_core::decode_config_bytes` (UTF-8 with or without BOM,
//! UTF-16 with BOM), shared with the TIP so `--check` and the IME always agree.

use pime_core::TextEncoding;

/// Encodes text for writing back. UTF-16 files are rewritten as UTF-8 with a BOM
/// (every editor reads that, and TOML itself is defined as UTF-8).
pub fn encode_config_text(text: &str, original: TextEncoding) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 3);
    if original != TextEncoding::Utf8 {
        out.extend_from_slice(b"\xEF\xBB\xBF");
    }
    out.extend_from_slice(text.as_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pime_core::decode_config_bytes;

    #[test]
    fn core_decoding_cases_used_here() {
        let d = decode_config_bytes(b"\xEF\xBB\xBFa = 1").unwrap();
        assert_eq!(
            (d.text.as_str(), d.encoding),
            ("a = 1", TextEncoding::Utf8Bom)
        );
        let mut le = vec![0xFF, 0xFE];
        for u in "v = [\"و\"]".encode_utf16() {
            le.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(
            decode_config_bytes(&le).unwrap().encoding,
            TextEncoding::Utf16Le
        );
        let issue = decode_config_bytes(&[0x80, 0x81, 0xFE]).unwrap_err();
        assert_eq!(
            pime_core::fallback_of(&[issue]),
            pime_core::Fallback::WholeFile
        );
    }

    #[test]
    fn encode_keeps_bom_choice() {
        assert_eq!(encode_config_text("x", TextEncoding::Utf8), b"x");
        assert_eq!(
            encode_config_text("x", TextEncoding::Utf8Bom),
            b"\xEF\xBB\xBFx"
        );
        assert_eq!(
            encode_config_text("x", TextEncoding::Utf16Le),
            b"\xEF\xBB\xBFx"
        );
    }

    #[test]
    fn round_trip_through_core() {
        for enc in [TextEncoding::Utf8, TextEncoding::Utf8Bom] {
            let bytes = encode_config_text("gh = \"qaf\"\n", enc);
            let d = decode_config_bytes(&bytes).unwrap();
            assert_eq!((d.text.as_str(), d.encoding), ("gh = \"qaf\"\n", enc));
        }
    }
}
