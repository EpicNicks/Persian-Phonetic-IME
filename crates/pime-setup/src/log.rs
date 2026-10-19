//! Best-effort log in `%TEMP%\PersianPhoneticIme\pime-setup.log` (for SYSTEM custom actions that is
//! the system temp folder). Only commands, paths, GUIDs and result codes are logged; this tool
//! never sees typed text. Logging failures are ignored.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

const MAX_BYTES: u64 = 512 * 1024;

pub struct Log {
    path: Option<PathBuf>,
    prefix: String,
}

/// `2026-10-17 13:05:09 pid=1234 x64 message`
pub fn format_line(stamp: [u16; 6], pid: u32, arch: &str, msg: &str) -> String {
    let [y, mo, d, h, mi, s] = stamp;
    // One line per entry, whatever the message contains.
    let msg: String = msg
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02} pid={pid} {arch} {msg}\n")
}

impl Log {
    pub fn open(arch: &str) -> Log {
        let path = std::env::var_os("TEMP")
            .or_else(|| std::env::var_os("TMP"))
            .map(|t| {
                let dir = PathBuf::from(t).join("PersianPhoneticIme");
                let _ = fs::create_dir_all(&dir);
                dir.join("pime-setup.log")
            });
        if let Some(p) = &path {
            if fs::metadata(p).is_ok_and(|m| m.len() > MAX_BYTES) {
                let _ = fs::remove_file(p);
            }
        }
        Log {
            path,
            prefix: arch.to_string(),
        }
    }

    /// A log that writes nothing (`/status` stays strictly read-only).
    pub fn none() -> Log {
        Log {
            path: None,
            prefix: String::new(),
        }
    }

    pub fn path(&self) -> Option<&PathBuf> {
        self.path.as_ref()
    }

    pub fn line(&self, msg: &str) {
        let Some(path) = &self.path else { return };
        let line = format_line(
            crate::sys::local_time(),
            std::process::id(),
            &self.prefix,
            msg,
        );
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_format() {
        assert_eq!(
            format_line([2026, 10, 17, 9, 5, 3], 42, "x64", "register ok"),
            "2026-10-17 09:05:03 pid=42 x64 register ok\n"
        );
        assert_eq!(
            format_line([2026, 1, 2, 3, 4, 5], 1, "x86", "a\r\nb"),
            "2026-01-02 03:04:05 pid=1 x86 a  b\n"
        );
    }

    #[test]
    fn none_writes_nothing() {
        let l = Log::none();
        assert!(l.path().is_none());
        l.line("ignored");
    }
}
