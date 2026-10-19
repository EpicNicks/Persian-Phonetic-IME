//! Learner hints shown under candidates when `candidate_labels = true`.

/// The everyday name of a Persian letter ("sin", "sad", "se"), if known.
pub fn candidate_label(candidate: &str) -> Option<&'static str> {
    Some(match candidate {
        "ا" => "alef",
        "آ" => "alef madde",
        "ب" => "be",
        "پ" => "pe",
        "ت" => "te",
        "ث" => "se",
        "ج" => "jim",
        "چ" => "che",
        "ح" => "he jimi",
        "خ" => "khe",
        "د" => "dal",
        "ذ" => "zal",
        "ر" => "re",
        "ز" => "ze",
        "ژ" => "zhe",
        "س" => "sin",
        "ش" => "shin",
        "ص" => "sad",
        "ض" => "zad",
        "ط" => "ta",
        "ظ" => "za",
        "ع" => "eyn",
        "غ" => "gheyn",
        "ف" => "fe",
        "ق" => "qaf",
        "ک" => "kaf",
        "گ" => "gaf",
        "ل" => "lam",
        "م" => "mim",
        "ن" => "nun",
        "و" => "vav",
        "ه" => "he",
        "ی" => "ye",
        "ء" => "hamze",
        "\u{200C}" => "half-space",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ResolvedConfig;

    #[test]
    fn every_default_candidate_has_a_label() {
        let cfg = ResolvedConfig::default();
        for key in cfg.keys() {
            for c in cfg.candidates(key).unwrap() {
                assert!(candidate_label(c).is_some(), "no label for {c}");
            }
        }
        assert_eq!(candidate_label("x"), None);
    }
}
