//! Embeds the built-in data files (`data/presets/*.toml`, `data/punctuation.toml`)
//! with `include_str!` when they exist, so the crate still builds (falling back
//! to its compiled-in defaults) when it is used without the repo's `data/` tree.

use std::fmt::Write as _;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let data = manifest.join("..").join("..").join("data");
    let presets_dir = data.join("presets");
    let punctuation = data.join("punctuation.toml");
    println!("cargo:rerun-if-changed={}", presets_dir.display());
    println!("cargo:rerun-if-changed={}", punctuation.display());
    println!("cargo:rerun-if-changed=build.rs");

    let mut presets: Vec<(String, PathBuf)> = std::fs::read_dir(&presets_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .filter_map(|p| {
            let stem = p.file_stem()?.to_str()?.to_string();
            Some((stem, p))
        })
        .collect();
    presets.sort();

    let mut out = String::new();
    out.push_str("/// Built-in presets found in `data/presets` at build time: (name, toml).\n");
    out.push_str("pub(crate) static DATA_PRESETS: &[(&str, &str)] = &[\n");
    for (name, path) in &presets {
        let path = std::fs::canonicalize(path).unwrap_or(path.clone());
        writeln!(
            out,
            "    ({:?}, include_str!({:?})),",
            name,
            path.display().to_string()
        )
        .unwrap();
    }
    out.push_str("];\n");
    if punctuation.is_file() {
        let path = std::fs::canonicalize(&punctuation).unwrap_or(punctuation.clone());
        writeln!(
            out,
            "pub(crate) static DATA_PUNCTUATION: Option<&str> = Some(include_str!({:?}));",
            path.display().to_string()
        )
        .unwrap();
    } else {
        out.push_str("pub(crate) static DATA_PUNCTUATION: Option<&str> = None;\n");
    }
    let dest = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("embedded_data.rs");
    std::fs::write(dest, out).unwrap();
}
