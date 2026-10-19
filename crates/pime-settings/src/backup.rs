//! Timestamped backups and "Reset to defaults".

use crate::defaults::DEFAULT_CONFIG;
use crate::edit::write_atomically;
use std::io;
use std::path::{Path, PathBuf};

/// A wall-clock time, used only to name backups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timestamp {
    /// Year.
    pub year: u32,
    /// 1-12.
    pub month: u32,
    /// 1-31.
    pub day: u32,
    /// 0-23.
    pub hour: u32,
    /// 0-59.
    pub minute: u32,
    /// 0-59.
    pub second: u32,
}

impl Timestamp {
    /// UTC time from seconds since the Unix epoch.
    pub fn from_unix(secs: u64) -> Timestamp {
        // Howard Hinnant's civil_from_days.
        let days = (secs / 86_400) as i64;
        let rem = secs % 86_400;
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        let year = (yoe + era * 400 + i64::from(month <= 2)) as u32;
        Timestamp {
            year,
            month,
            day,
            hour: (rem / 3600) as u32,
            minute: (rem / 60 % 60) as u32,
            second: (rem % 60) as u32,
        }
    }

    /// The current local time (UTC where local time is not available).
    pub fn now() -> Timestamp {
        #[cfg(windows)]
        {
            // SAFETY: GetLocalTime only writes the returned struct.
            let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
            Timestamp {
                year: t.wYear.into(),
                month: t.wMonth.into(),
                day: t.wDay.into(),
                hour: t.wHour.into(),
                minute: t.wMinute.into(),
                second: t.wSecond.into(),
            }
        }
        #[cfg(not(windows))]
        {
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            Timestamp::from_unix(secs)
        }
    }

    /// `YYYYMMDD-HHMMSS` (sorts chronologically, valid in file names).
    pub fn compact(&self) -> String {
        format!(
            "{:04}{:02}{:02}-{:02}{:02}{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
}

/// `config.toml.bak-<timestamp>` next to `config`, with `-2`, `-3`, ... appended if
/// that name is already taken (two resets in the same second).
pub fn backup_path(config: &Path, at: Timestamp) -> PathBuf {
    let name = config
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let base = format!("{name}.bak-{}", at.compact());
    let mut candidate = config.with_file_name(&base);
    let mut n = 2;
    while candidate.exists() {
        candidate = config.with_file_name(format!("{base}-{n}"));
        n += 1;
    }
    candidate
}

/// Copies `config` to a fresh backup and returns its path; `None` if there is no
/// file to back up.
pub fn backup(config: &Path, at: Timestamp) -> io::Result<Option<PathBuf>> {
    if !config.exists() {
        return Ok(None);
    }
    let dest = backup_path(config, at);
    std::fs::copy(config, &dest)?;
    Ok(Some(dest))
}

/// Backs up `config` (if present) and replaces it with the commented default.
/// Returns the backup path. Does not signal the reload event.
pub fn reset_config(config: &Path, at: Timestamp) -> io::Result<Option<PathBuf>> {
    let saved = backup(config, at)?;
    if let Some(dir) = config.parent() {
        std::fs::create_dir_all(dir)?;
    }
    write_atomically(config, DEFAULT_CONFIG.as_bytes())?;
    Ok(saved)
}

/// Writes the commented default if `config` does not exist yet. Returns whether it
/// was created.
pub fn ensure_exists(config: &Path) -> io::Result<bool> {
    if config.exists() {
        return Ok(false);
    }
    if let Some(dir) = config.parent() {
        std::fs::create_dir_all(dir)?;
    }
    write_atomically(config, DEFAULT_CONFIG.as_bytes())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    const AT: Timestamp = Timestamp {
        year: 2026,
        month: 10,
        day: 7,
        hour: 9,
        minute: 5,
        second: 3,
    };

    #[test]
    fn unix_conversion() {
        assert_eq!(Timestamp::from_unix(0).compact(), "19700101-000000");
        assert_eq!(
            Timestamp::from_unix(951_782_400).compact(),
            "20000229-000000"
        );
        assert_eq!(
            Timestamp::from_unix(1_792_195_199).compact(),
            "20261016-235959"
        );
    }

    #[test]
    fn backup_names_do_not_collide() {
        let dir = TempDir::new("bak");
        let cfg = dir.path().join("config.toml");
        std::fs::write(&cfg, "x").unwrap();
        let a = backup(&cfg, AT).unwrap().unwrap();
        let b = backup(&cfg, AT).unwrap().unwrap();
        assert_eq!(a.file_name().unwrap(), "config.toml.bak-20261007-090503");
        assert_eq!(b.file_name().unwrap(), "config.toml.bak-20261007-090503-2");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "x");
    }

    #[test]
    fn reset_backs_up_and_writes_default() {
        let dir = TempDir::new("reset");
        let cfg = dir.path().join("config.toml");
        std::fs::write(&cfg, "[options]\ngh = \"kaf\"\n").unwrap();
        let saved = reset_config(&cfg, AT).unwrap().unwrap();
        assert_eq!(
            std::fs::read_to_string(saved).unwrap(),
            "[options]\ngh = \"kaf\"\n"
        );
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), DEFAULT_CONFIG);

        let fresh = dir.path().join("new").join("config.toml");
        assert_eq!(reset_config(&fresh, AT).unwrap(), None);
        assert!(fresh.exists());
    }

    #[test]
    fn ensure_exists_only_creates_once() {
        let dir = TempDir::new("ensure");
        let cfg = dir.path().join("config.toml");
        assert!(ensure_exists(&cfg).unwrap());
        std::fs::write(&cfg, "mine").unwrap();
        assert!(!ensure_exists(&cfg).unwrap());
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), "mine");
    }
}
