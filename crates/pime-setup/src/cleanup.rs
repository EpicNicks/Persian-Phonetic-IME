//! Planning for `/cleanup-old` (pure). After an upgrade the old version folder may still hold a
//! TIP DLL loaded by running apps; such files are scheduled for deletion at reboot. The planner is
//! deliberately narrow: it only ever touches our own file names, directly inside version-named
//! folders, directly under a folder named exactly [`PRODUCT_DIR_NAME`].

use std::path::Path;

/// `%ProgramFiles%\Persian Phonetic IME` (installer/Package.wxs).
pub const PRODUCT_DIR_NAME: &str = "Persian Phonetic IME";

/// The root passed by the installer must be our product folder (trailing separators allowed).
pub fn is_product_root(root: &Path) -> bool {
    root.is_absolute()
        && root
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n == PRODUCT_DIR_NAME)
}

/// `1.2.3` or `1.2.3.4`, digits only.
pub fn is_version_name(name: &str) -> bool {
    let parts: Vec<&str> = name.split('.').collect();
    (3..=4).contains(&parts.len())
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()))
}

/// Version folders other than `keep`.
pub fn stale_version_dirs<'a>(names: &[&'a str], keep: &str) -> Vec<&'a str> {
    names
        .iter()
        .copied()
        .filter(|n| is_version_name(n) && !n.eq_ignore_ascii_case(keep))
        .collect()
}

/// Files the installer ships (and nothing else): versioned TIP DLLs (+pdb), helper, settings and the shared dictionary + notices.
/// Per-user data (history.bin, config) never lives here and is never matched.
pub fn is_our_file(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    let tip = n.starts_with("persianphoneticime.")
        && (n.ends_with(".x64.dll")
            || n.ends_with(".x86.dll")
            || n.ends_with(".x64.pdb")
            || n.ends_with(".x86.pdb"));
    let exe = matches!(
        n.as_str(),
        "pime-setup.x64.exe"
            | "pime-setup.x86.exe"
            | "pime-settings.exe"
            | "placeholder.txt"
            | "dict.bin"
            | "third-party-notices.txt"
    );
    tip || exe
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_root() {
        assert!(is_product_root(Path::new(
            r"C:\Program Files\Persian Phonetic IME"
        )));
        assert!(is_product_root(Path::new(
            r"C:\Program Files\Persian Phonetic IME\"
        )));
        // The MSI passes "[PRODUCTROOT]." so the trailing backslash cannot escape the quote.
        assert!(is_product_root(Path::new(
            r"C:\Program Files\Persian Phonetic IME\."
        )));
        assert!(!is_product_root(Path::new(r"C:\Program Files")));
        assert!(!is_product_root(Path::new(r"C:\")));
        assert!(!is_product_root(Path::new(r"Persian Phonetic IME")));
        assert!(!is_product_root(Path::new(
            r"C:\Program Files\Persian Phonetic IME\0.1.0"
        )));
    }

    #[test]
    fn versions() {
        for ok in ["0.1.0", "1.2.3.4", "10.0.65535"] {
            assert!(is_version_name(ok), "{ok}");
        }
        for bad in [
            "",
            "1",
            "1.2",
            "1.2.3.4.5",
            "1..3",
            "v1.2.3",
            "1.2.3-beta",
            "..",
            "1.2.x",
        ] {
            assert!(!is_version_name(bad), "{bad}");
        }
        assert_eq!(
            stale_version_dirs(&["0.1.0", "0.2.0", "logs", "0.3.0", "x.y.z"], "0.2.0"),
            vec!["0.1.0", "0.3.0"]
        );
        assert!(stale_version_dirs(&["0.2.0"], "0.2.0").is_empty());
    }

    #[test]
    fn our_files_only() {
        for ok in [
            "PersianPhoneticIme.0.1.0.x64.dll",
            "PersianPhoneticIme.0.1.0.x86.dll",
            "persianphoneticime.0.1.0.x64.pdb",
            "pime-setup.x64.exe",
            "pime-setup.x86.exe",
            "pime-settings.exe",
            "PLACEHOLDER.txt",
            "dict.bin",
            "THIRD-PARTY-NOTICES.txt",
        ] {
            assert!(is_our_file(ok), "{ok}");
        }
        for bad in [
            "config.toml",
            "history.bin",
            "notes.txt",
            "PersianPhoneticIme.dll",
            "evil.x64.dll",
            "x.exe",
        ] {
            assert!(!is_our_file(bad), "{bad}");
        }
    }
}
