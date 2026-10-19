//! Text-service runtime switches, read once per activation from
//! `HKCU\Software\PersianPhoneticIme\Tip` (DWORD values). These are host-compatibility
//! switches for the sandbox matrix, not user preferences (those live in config.toml).

/// See module docs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// `EatPrintable` (default 1). Idle printable keys (digits, punctuation, Space) are eaten and
    /// inserted by us (converted per config, else from the US-QWERTY table), so the underlying
    /// Persian layout never translates them (PLAN 2.4, docs/spike.md 4.4). 0 = let them pass
    /// through to the layout. Also governs the Latin ("A") mode: 1 = insert US-QWERTY
    /// characters, 0 = pure pass-through.
    pub eat_printable: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            eat_printable: true,
        }
    }
}

impl Settings {
    /// Builds settings from a value lookup (`None` = value absent, keep the default).
    pub fn from_values(get: impl Fn(&str) -> Option<u32>) -> Self {
        let d = Settings::default();
        Settings {
            eat_printable: get("EatPrintable").map_or(d.eat_printable, |v| v != 0),
        }
    }

    /// Reads HKCU. Never fails: missing values keep their defaults.
    pub fn read() -> Self {
        use windows::Win32::System::Registry::HKEY_CURRENT_USER;
        Self::from_values(|name| {
            crate::registry::read_dword(HKEY_CURRENT_USER, crate::registry::TIP_KEY, name)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_overrides() {
        assert!(Settings::from_values(|_| None).eat_printable);
        assert!(!Settings::from_values(|n| (n == "EatPrintable").then_some(0)).eat_printable);
        assert!(Settings::from_values(|n| (n == "EatPrintable").then_some(7)).eat_printable);
    }
}
