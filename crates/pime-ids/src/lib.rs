//! Product GUIDs of Persian Phonetic IME: the single source of truth.
//!
//! **FROZEN once a release ships.** Changing any value orphans existing installs: the CLSID and
//! profile GUID are stored in HKLM, in every user's input-method list and in the MSI's registry
//! searches; the upgrade code ties MSI versions together. Generated fresh on 2026-10-17 (WS8);
//! deliberately different from the spike's GUIDs (`spike/src/guids.rs`) so a leftover spike
//! registration can never shadow the product.
//!
//! This crate has no dependencies on purpose; `pime-setup` and `pime-tip` both depend on it.
//! With windows-rs: `GUID::from_u128(CLSID_TEXT_SERVICE)`.
//! `installer/build.ps1` reads the CLSID and profile GUID from this file for the WiX build.

/// `CLSID_PersianPhoneticTextService` (COM class of the TIP DLL, x64 and x86 share it).
pub const CLSID_TEXT_SERVICE: u128 = 0x49b3a09b_d707_4675_b4eb_62b17ccb6d73;
/// `GUID_Profile_FaPhonetic`: the fa-IR language profile registered with RegisterProfile.
pub const GUID_PROFILE_FA_PHONETIC: u128 = 0xc1074472_386f_4d95_ab2d_887eb94bd96d;
/// Display attribute: normal composing text (underline).
pub const GUID_DISPLAY_ATTR_INPUT: u128 = 0x80321f57_87bf_47a0_a448_f3ea02addf48;
/// Display attribute: pending Latin run (lone `c`), dotted grey.
pub const GUID_DISPLAY_ATTR_PENDING: u128 = 0x01556899_38bf_4da4_a337_f2b4fef8f807;
/// Display attribute: selected/converted segment (thick underline).
pub const GUID_DISPLAY_ATTR_SELECTED: u128 = 0x85efbd51_e498_4aeb_a75a_d10e7a3427d5;
/// Language bar / input indicator button (`ITfLangBarItemButton`, "Ù"/"A").
pub const GUID_LANGBAR_INPUT_MODE: u128 = 0x662597ca_25f6_4b99_a164_14316994fcf5;
/// Preserved key for the optional on/off hotkey (`ITfKeystrokeMgr::PreserveKey`).
pub const GUID_PRESERVED_KEY_TOGGLE: u128 = 0x83d109f1_b1fb_41d0_a417_ead2ed397ea8;
/// Active Setup component id (per-user enable at logon, opt-in, see installer/README.md).
pub const ACTIVE_SETUP_ID: u128 = 0x46cb163b_5a81_40c6_ab63_b56299af171c;
/// MSI UpgradeCode (installer/Package.wxs). Kept here so every frozen GUID lives in one file.
pub const MSI_UPGRADE_CODE: u128 = 0x320151be_756e_46c6_b2e6_5ced4ed6a2ab;

/// fa-IR.
pub const LANGID_FA_IR: u16 = 0x0429;
/// Name shown in the input-method list.
pub const PROFILE_DESCRIPTION: &str = "Persian Phonetic";

/// Every frozen GUID with its name, for tests and `/status`.
pub const ALL: [(&str, u128); 9] = [
    ("CLSID_TEXT_SERVICE", CLSID_TEXT_SERVICE),
    ("GUID_PROFILE_FA_PHONETIC", GUID_PROFILE_FA_PHONETIC),
    ("GUID_DISPLAY_ATTR_INPUT", GUID_DISPLAY_ATTR_INPUT),
    ("GUID_DISPLAY_ATTR_PENDING", GUID_DISPLAY_ATTR_PENDING),
    ("GUID_DISPLAY_ATTR_SELECTED", GUID_DISPLAY_ATTR_SELECTED),
    ("GUID_LANGBAR_INPUT_MODE", GUID_LANGBAR_INPUT_MODE),
    ("GUID_PRESERVED_KEY_TOGGLE", GUID_PRESERVED_KEY_TOGGLE),
    ("ACTIVE_SETUP_ID", ACTIVE_SETUP_ID),
    ("MSI_UPGRADE_CODE", MSI_UPGRADE_CODE),
];

/// Registry form: `{49B3A09B-D707-4675-B4EB-62B17CCB6D73}` (upper case, braced).
pub fn braced(g: u128) -> String {
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:04X}-{:012X}}}",
        (g >> 96) as u32,
        (g >> 80) as u16,
        (g >> 64) as u16,
        (g >> 48) as u16,
        g & 0xffff_ffff_ffff
    )
}

/// Parses `{xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}` (braces optional, any case).
pub fn parse(s: &str) -> Option<u128> {
    let s = s.trim();
    let s = s
        .strip_prefix('{')
        .and_then(|t| t.strip_suffix('}'))
        .unwrap_or(s);
    let groups: Vec<&str> = s.split('-').collect();
    let lens = [8, 4, 4, 4, 12];
    if groups.len() != 5 || groups.iter().zip(lens).any(|(g, n)| g.len() != n) {
        return None;
    }
    let hex: String = groups.concat();
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u128::from_str_radix(&hex, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frozen_values() {
        // If this test fails, a frozen GUID was edited. Do not "fix" the test: revert the GUID.
        assert_eq!(
            braced(CLSID_TEXT_SERVICE),
            "{49B3A09B-D707-4675-B4EB-62B17CCB6D73}"
        );
        assert_eq!(
            braced(GUID_PROFILE_FA_PHONETIC),
            "{C1074472-386F-4D95-AB2D-887EB94BD96D}"
        );
        assert_eq!(
            braced(MSI_UPGRADE_CODE),
            "{320151BE-756E-46C6-B2E6-5CED4ED6A2AB}"
        );
        assert_eq!(
            braced(ACTIVE_SETUP_ID),
            "{46CB163B-5A81-40C6-AB63-B56299AF171C}"
        );
        assert_eq!(LANGID_FA_IR, 0x0429);
    }

    #[test]
    fn all_distinct_nonzero_and_v4() {
        for (i, (name, a)) in ALL.iter().enumerate() {
            assert_ne!(*a, 0, "{name}");
            // version nibble 4 and RFC 4122 variant: these are random GUIDs, not hand-made ones
            assert_eq!((a >> 76) & 0xf, 4, "{name} version");
            assert_eq!((a >> 62) & 0x3, 0b10, "{name} variant");
            for (other, b) in &ALL[i + 1..] {
                assert_ne!(a, b, "{name} == {other}");
            }
        }
    }

    #[test]
    fn differs_from_spike() {
        let spike = [
            0x674850a2_4c3b_46fa_9b17_3ca5465ffa18u128,
            0xda9a94b1_be61_4ff6_a706_7feaf1e62a33,
            0x802ad415_64ab_4cc4_832e_06a2666f27bc,
        ];
        for (name, g) in ALL {
            assert!(!spike.contains(&g), "{name} reuses a spike GUID");
        }
    }

    #[test]
    fn braced_parse_round_trip() {
        for (_, g) in ALL {
            assert_eq!(parse(&braced(g)), Some(g));
            assert_eq!(parse(&braced(g).to_lowercase()), Some(g));
            assert_eq!(
                parse(braced(g).trim_matches(|c| c == '{' || c == '}')),
                Some(g)
            );
        }
        assert_eq!(
            braced(0x0000_0001_0002_0003_0004_0000_0000_0005),
            "{00000001-0002-0003-0004-000000000005}"
        );
    }

    #[test]
    fn parse_rejects_garbage() {
        for s in [
            "",
            "{}",
            "{49B3A09B-D707-4675-B4EB-62B17CCB6D7}",
            "{49B3A09B-D707-4675-B4EB-62B17CCB6D733}",
            "{49B3A09BD707-4675-B4EB-62B17CCB6D73}",
            "{49B3A09B-D707-4675-B4EB-62B17CCB6DZ3}",
            "{+9B3A09B-D707-4675-B4EB-62B17CCB6D73}",
            "{49B3A09B-D707-4675-B4EB-62B17CCB6D73",
        ] {
            assert_eq!(parse(s), None, "{s:?}");
        }
    }
}
