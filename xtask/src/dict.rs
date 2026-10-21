//! `build-dict` and `check-dict`: thin wrappers over `pime_dict` (docs/autocomplete.md 3.2).
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use pime_dict::{build_from_tsv, BuildOptions, Dictionary};

/// INFO lines recorded in dict.bin unless overridden with `--info` (see data/dict/SOURCE.txt).
const DEFAULT_INFO: [(&str, &str); 5] = [
    ("source", "wordfreq"),
    ("wordfreq_version", "3.1.1"),
    ("wordlist", "small"),
    ("licence", "CC-BY-SA-4.0"),
    ("builder", "xtask build-dict"),
];

#[derive(Debug, PartialEq)]
pub struct BuildArgs {
    pub inputs: Vec<PathBuf>,
    pub out: PathBuf,
    pub min_zipf: Option<f32>,
    pub max_entries: Option<usize>,
    pub max_bytes: Option<usize>,
    pub info: Vec<(String, String)>,
}

fn value<'a>(it: &mut impl Iterator<Item = &'a String>, flag: &str) -> Result<&'a String, String> {
    it.next().ok_or_else(|| format!("{flag} needs a value"))
}

fn number<T: std::str::FromStr>(s: &str, flag: &str) -> Result<T, String> {
    s.parse()
        .map_err(|_| format!("{flag}: invalid number '{s}'"))
}

pub fn parse_build_args(args: &[String]) -> Result<BuildArgs, String> {
    let mut a = BuildArgs {
        inputs: Vec::new(),
        out: PathBuf::new(),
        min_zipf: None,
        max_entries: None,
        max_bytes: None,
        info: Vec::new(),
    };
    let mut have_out = false;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--input" => a.inputs.push(PathBuf::from(value(&mut it, flag)?)),
            "--out" => {
                a.out = PathBuf::from(value(&mut it, flag)?);
                have_out = true;
            }
            "--min-zipf" => a.min_zipf = Some(number(value(&mut it, flag)?, flag)?),
            "--max-entries" => a.max_entries = Some(number(value(&mut it, flag)?, flag)?),
            "--max-bytes" => a.max_bytes = Some(number(value(&mut it, flag)?, flag)?),
            "--info" => {
                let kv = value(&mut it, flag)?;
                let (k, v) = kv.split_once('=').ok_or("--info expects key=value")?;
                a.info.push((k.to_string(), v.to_string()));
            }
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    if a.inputs.is_empty() {
        return Err("build-dict: at least one --input is required".to_string());
    }
    if !have_out {
        return Err("build-dict: --out is required".to_string());
    }
    Ok(a)
}

pub fn build(args: &[String]) -> Result<(), String> {
    let a = parse_build_args(args)?;
    let mut readers = Vec::new();
    for p in &a.inputs {
        let f = File::open(p).map_err(|e| format!("cannot open {}: {e}", p.display()))?;
        readers.push(BufReader::new(f));
    }
    let mut refs: Vec<&mut dyn std::io::BufRead> = readers
        .iter_mut()
        .map(|r| r as &mut dyn std::io::BufRead)
        .collect();

    let mut info = a.info;
    for (k, v) in DEFAULT_INFO {
        if !info.iter().any(|(ik, _)| ik == k) {
            info.push((k.to_string(), v.to_string()));
        }
    }
    // Contract defaults (docs/autocomplete.md 3.2), spelled out so no `Default` impl is assumed.
    let opts = BuildOptions {
        min_zipf: a.min_zipf.unwrap_or(1.0),
        max_entries: a.max_entries.unwrap_or(200_000),
        max_bytes: a.max_bytes.unwrap_or(16 << 20),
        info,
    };

    let (bytes, report) =
        build_from_tsv(&mut refs, &opts).map_err(|e| format!("build failed: {e:?}"))?;
    if let Some(dir) = a.out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    std::fs::write(&a.out, &bytes).map_err(|e| format!("cannot write {}: {e}", a.out.display()))?;
    println!("{report:#?}");
    println!("wrote {} ({} bytes)", a.out.display(), bytes.len());
    Ok(())
}

pub fn check(args: &[String]) -> Result<(), String> {
    let [path] = args else {
        return Err("usage: xtask check-dict <path>".to_string());
    };
    let bytes = std::fs::read(Path::new(path)).map_err(|e| format!("cannot read {path}: {e}"))?;
    let size = bytes.len();
    let dict = Dictionary::from_bytes(bytes).map_err(|e| format!("invalid dictionary: {e:?}"))?;
    println!("OK: {} entries, {size} bytes", dict.len());
    println!("{}", dict.info());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parses_repeated_inputs() {
        let a = parse_build_args(&s(&[
            "--input", "a.tsv", "--input", "b.tsv", "--out", "d.bin",
        ]))
        .unwrap();
        assert_eq!(a.inputs.len(), 2);
        assert_eq!(a.out, PathBuf::from("d.bin"));
    }

    #[test]
    fn rejects_bad_args() {
        assert!(parse_build_args(&s(&["--out", "d.bin"])).is_err());
        assert!(parse_build_args(&s(&["--input", "a"])).is_err());
        assert!(parse_build_args(&s(&["--input", "a", "--out", "b", "--x"])).is_err());
        assert!(parse_build_args(&s(&["--input", "a", "--out", "b", "--min-zipf", "q"])).is_err());
    }
}
