//! Pure helpers: InstallLayoutOrTip strings and the registry paths `/status` inspects.

use crate::guids::braced;

/// `ILOT_UNINSTALL` (input.dll, not in public headers; docs/spike.md 4.7).
pub const ILOT_UNINSTALL: u32 = 0x1;
/// Enable: no flags (add to the user's list, apply to the current session).
pub const ILOT_ENABLE: u32 = 0x0;

/// The documentation shows two spellings of the LangID prefix; which one input.dll accepts is
/// unverified (docs/spike.md 3, row "InstallLayoutOrTip psz spelling"), so both are tried.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LangSpelling {
    /// `0429:{CLSID}{Profile}`
    Plain,
    /// `0x0429:{CLSID}{Profile}`
    Hex,
}

/// Order in which `/enable-user` and `/disable-user` try the spellings.
pub const SPELLINGS: [LangSpelling; 2] = [LangSpelling::Plain, LangSpelling::Hex];

/// The TIP descriptor InstallLayoutOrTip expects: `<LangID>:{CLSID}{ProfileGUID}`.
pub fn tip_string(lang: u16, clsid: u128, profile: u128, spelling: LangSpelling) -> String {
    let prefix = match spelling {
        LangSpelling::Plain => "",
        LangSpelling::Hex => "0x",
    };
    format!("{prefix}{lang:04X}:{}{}", braced(clsid), braced(profile))
}

/// `Software\Classes\CLSID\{clsid}\InprocServer32` under HKLM (redirected per bitness view).
pub fn inproc_key(clsid: u128) -> String {
    format!(r"Software\Classes\CLSID\{}\InprocServer32", braced(clsid))
}

/// `SOFTWARE\Microsoft\CTF\TIP\{clsid}` (HKLM: registration; HKCU: per-user TSF state).
pub fn tip_key(clsid: u128) -> String {
    format!(r"SOFTWARE\Microsoft\CTF\TIP\{}", braced(clsid))
}

/// The fa-IR language profile written by RegisterProfile.
pub fn language_profile_key(clsid: u128, lang: u16, profile: u128) -> String {
    format!(
        r"{}\LanguageProfile\0x{:08x}\{}",
        tip_key(clsid),
        lang as u32,
        braced(profile)
    )
}

/// Per-user language list (Windows 10/11): one subkey per language tag, values named like the
/// TIP string. Matching is by substring so either spelling (and any case) is found.
pub const USER_PROFILE_KEY: &str = r"Control Panel\International\User Profile";

/// Does a value name in the user's language list refer to our TIP?
pub fn is_our_list_entry(value_name: &str, clsid: u128, profile: u128) -> bool {
    let v = value_name.to_ascii_uppercase();
    v.contains(&braced(clsid)) && v.contains(&braced(profile))
}

/// Kill switch path (PLAN 0), checked under HKCU and HKLM.
pub const KILL_SWITCH_KEY: &str = r"Software\PersianPhoneticIme";
pub const KILL_SWITCH_VALUE: &str = "Disabled";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guids::{CLSID_TEXT_SERVICE as C, GUID_PROFILE_FA_PHONETIC as P, LANGID_FA_IR};

    #[test]
    fn tip_strings() {
        assert_eq!(
            tip_string(LANGID_FA_IR, C, P, LangSpelling::Plain),
            "0429:{49B3A09B-D707-4675-B4EB-62B17CCB6D73}{C1074472-386F-4D95-AB2D-887EB94BD96D}"
        );
        assert_eq!(
            tip_string(LANGID_FA_IR, C, P, LangSpelling::Hex),
            "0x0429:{49B3A09B-D707-4675-B4EB-62B17CCB6D73}{C1074472-386F-4D95-AB2D-887EB94BD96D}"
        );
        assert_eq!(SPELLINGS[0], LangSpelling::Plain);
        assert_eq!(
            tip_string(0x0409, 1, 2, LangSpelling::Plain).len(),
            5 + 38 * 2
        );
    }

    #[test]
    fn registry_paths() {
        assert_eq!(
            inproc_key(C),
            r"Software\Classes\CLSID\{49B3A09B-D707-4675-B4EB-62B17CCB6D73}\InprocServer32"
        );
        assert_eq!(
            language_profile_key(C, LANGID_FA_IR, P),
            r"SOFTWARE\Microsoft\CTF\TIP\{49B3A09B-D707-4675-B4EB-62B17CCB6D73}\LanguageProfile\0x00000429\{C1074472-386F-4D95-AB2D-887EB94BD96D}"
        );
    }

    #[test]
    fn list_entry_matching() {
        let plain = tip_string(LANGID_FA_IR, C, P, LangSpelling::Plain);
        assert!(is_our_list_entry(&plain, C, P));
        assert!(is_our_list_entry(&plain.to_lowercase(), C, P));
        assert!(!is_our_list_entry("0429:00000429", C, P));
        assert!(!is_our_list_entry(
            &tip_string(LANGID_FA_IR, C, 7, LangSpelling::Plain),
            C,
            P
        ));
    }
}
