//! `xtask eval-dict`: completion quality and latency of a `dict.bin` (docs/autocomplete.md 9).

use pime_dict::eval::{
    evaluate_adaptive, evaluate_static, parse_corpus, parse_set, AdaptiveOptions, StaticOptions,
};
use pime_dict::Dictionary;
use std::path::PathBuf;

const USAGE: &str =
    "usage: xtask eval-dict --dict <dict.bin> --set <words.tsv> [--corpus <corpus.txt>]
                       [--min-chars N] [--passes N] [--json]";

pub fn run(args: &[String]) -> Result<(), String> {
    let mut dict = None;
    let mut set = None;
    let mut corpus = None;
    let mut json = false;
    let mut st = StaticOptions::default();
    let mut ad = AdaptiveOptions::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{a} needs a value\n{USAGE}"))
        };
        match a.as_str() {
            "--dict" => dict = Some(PathBuf::from(val()?)),
            "--set" => set = Some(PathBuf::from(val()?)),
            "--corpus" => corpus = Some(PathBuf::from(val()?)),
            "--min-chars" => {
                let n: usize = val()?.parse().map_err(|_| "--min-chars: not a number")?;
                st.min_letters = n.max(1);
                ad.min_letters = n.max(1);
            }
            "--passes" => ad.passes = val()?.parse().map_err(|_| "--passes: not a number")?,
            "--json" => json = true,
            _ => return Err(format!("unknown argument {a:?}\n{USAGE}")),
        }
    }
    let (Some(dict_path), Some(set_path)) = (dict, set) else {
        return Err(USAGE.to_string());
    };
    let dict = Dictionary::load(&dict_path)
        .map_err(|e| format!("cannot load {}: {e:?}", dict_path.display()))?;
    let set_text = std::fs::read_to_string(&set_path)
        .map_err(|e| format!("cannot read {}: {e}", set_path.display()))?;
    let words = parse_set(&set_text);
    let report = evaluate_static(&dict, &words, &st);
    let adaptive = match &corpus {
        Some(p) => {
            let text = std::fs::read_to_string(p)
                .map_err(|e| format!("cannot read {}: {e}", p.display()))?;
            Some(evaluate_adaptive(&dict, &parse_corpus(&text), &ad))
        }
        None => None,
    };
    if json {
        let ad = adaptive
            .as_ref()
            .map_or("null".to_string(), |a| a.to_json());
        println!(
            "{{\"dict_words\":{},\"release\":{},\"static\":{},\"adaptive\":{}}}",
            dict.len(),
            !cfg!(debug_assertions),
            report.to_json(),
            ad
        );
    } else {
        println!("dictionary: {} words", dict.len());
        print!("{}", report.to_text());
        if let Some(a) = &adaptive {
            print!("{}", a.to_text());
        }
        if cfg!(debug_assertions) {
            println!("note: debug build; measure latency with --release");
        }
    }
    Ok(())
}
