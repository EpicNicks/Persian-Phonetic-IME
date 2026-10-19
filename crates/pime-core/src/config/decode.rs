//! Turning the bytes of `config.toml` into text, shared by the IME and the
//! settings app so both always read the file the same way.
//!
//! Notepad may save UTF-8 with a byte-order mark or "Unicode" (UTF-16 LE with a
//! BOM); both are accepted. Anything that is not valid text in a supported
//! encoding is one whole-file problem, and the defaults stay in use.

use super::{load_config, ConfigIssue, IssueKind, LoadedConfig, ResolvedConfig, Severity};

/// How the file was stored, so a rewrite can keep the user's choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEncoding {
    /// UTF-8 without a byte-order mark.
    Utf8,
    /// UTF-8 starting with EF BB BF.
    Utf8Bom,
    /// UTF-16 little-endian with a BOM (Notepad's "Unicode").
    Utf16Le,
    /// UTF-16 big-endian with a BOM.
    Utf16Be,
}

/// Decoded file text (without the BOM) and how it was stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedConfig {
    /// The text.
    pub text: String,
    /// The encoding it was stored in.
    pub encoding: TextEncoding,
}

/// Decodes config file bytes. `Err` carries the whole-file issue to report
/// (kind [`IssueKind::Syntax`], so [`super::fallback_of`] says `WholeFile`).
pub fn decode_config_bytes(bytes: &[u8]) -> Result<DecodedConfig, ConfigIssue> {
    let decoded = if let Some(rest) = bytes.strip_prefix(b"\xEF\xBB\xBF") {
        std::str::from_utf8(rest)
            .ok()
            .map(|t| (t.to_string(), TextEncoding::Utf8Bom))
    } else if let Some(rest) = bytes.strip_prefix(b"\xFF\xFE") {
        utf16(rest, u16::from_le_bytes).map(|t| (t, TextEncoding::Utf16Le))
    } else if let Some(rest) = bytes.strip_prefix(b"\xFE\xFF") {
        utf16(rest, u16::from_be_bytes).map(|t| (t, TextEncoding::Utf16Be))
    } else {
        std::str::from_utf8(bytes)
            .ok()
            .map(|t| (t.to_string(), TextEncoding::Utf8))
    };
    match decoded {
        Some((text, encoding)) => Ok(DecodedConfig { text, encoding }),
        None => Err(not_text_issue()),
    }
}

/// Decodes and loads config file bytes in one step: undecodable bytes give the
/// default configuration plus the single whole-file issue.
pub fn load_config_bytes(bytes: &[u8]) -> LoadedConfig {
    match decode_config_bytes(bytes) {
        Ok(decoded) => load_config(&decoded.text),
        Err(issue) => LoadedConfig {
            config: ResolvedConfig::default(),
            issues: vec![issue],
        },
    }
}

fn utf16(rest: &[u8], unit: fn([u8; 2]) -> u16) -> Option<String> {
    if !rest.len().is_multiple_of(2) {
        return None;
    }
    let units: Vec<u16> = rest.chunks_exact(2).map(|c| unit([c[0], c[1]])).collect();
    String::from_utf16(&units).ok()
}

fn not_text_issue() -> ConfigIssue {
    ConfigIssue {
        severity: Severity::Error,
        kind: IssueKind::Syntax,
        line: 0,
        column: 0,
        snippet: String::new(),
        message: "this file couldn't be read (it isn't saved as plain text).".into(),
        hint: "Open it in Notepad and save it with the encoding \"UTF-8\", or reset it to the \
               defaults."
            .into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{fallback_of, format_issues, Digits, Fallback};

    #[test]
    fn plain_and_bom_utf8() {
        let d = decode_config_bytes("digits = \"ascii\"".as_bytes()).unwrap();
        assert_eq!(d.encoding, TextEncoding::Utf8);
        let d = decode_config_bytes(b"\xEF\xBB\xBFa = 1").unwrap();
        assert_eq!(
            (d.text.as_str(), d.encoding),
            ("a = 1", TextEncoding::Utf8Bom)
        );
        assert_eq!(decode_config_bytes(b"").unwrap().text, "");
    }

    #[test]
    fn utf16_both_orders() {
        let s = "v = [\"و\"]";
        let mut le = vec![0xFF, 0xFE];
        let mut be = vec![0xFE, 0xFF];
        for u in s.encode_utf16() {
            le.extend_from_slice(&u.to_le_bytes());
            be.extend_from_slice(&u.to_be_bytes());
        }
        assert_eq!(decode_config_bytes(&le).unwrap().text, s);
        let d = decode_config_bytes(&be).unwrap();
        assert_eq!((d.text.as_str(), d.encoding), (s, TextEncoding::Utf16Be));
        assert!(
            decode_config_bytes(&le[..le.len() - 1]).is_err(),
            "odd length"
        );
        assert!(
            decode_config_bytes(b"\xFF\xFE\x00\xD8").is_err(),
            "lone surrogate"
        );
    }

    #[test]
    fn binary_is_a_whole_file_problem() {
        let issue = decode_config_bytes(&[0x80, 0x81, 0xFE]).unwrap_err();
        let issues = [issue];
        assert_eq!(fallback_of(&issues), Fallback::WholeFile);
        let text = format_issues(&issues);
        assert!(
            text.starts_with("config.toml: this file couldn't be read"),
            "{text}"
        );
        assert!(text.contains("default settings are in use"), "{text}");
        assert!(
            decode_config_bytes(b"\xEF\xBB\xBF\xC0").is_err(),
            "bad UTF-8 after BOM"
        );
    }

    #[test]
    fn load_bytes_end_to_end() {
        let mut le = vec![0xFF, 0xFE];
        for u in "[options]\ndigits = \"ascii\"\n".encode_utf16() {
            le.extend_from_slice(&u.to_le_bytes());
        }
        let loaded = load_config_bytes(&le);
        assert!(loaded.issues.is_empty());
        assert_eq!(loaded.config.options().digits, Digits::Ascii);
        let loaded = load_config_bytes(&[0xFF]);
        assert_eq!(loaded.issues.len(), 1);
        assert_eq!(loaded.config, ResolvedConfig::default());
    }
}
