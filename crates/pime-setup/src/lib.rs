//! Persian Phonetic IME setup helper (library part: pure logic plus thin Win32 wrappers).
//! The binary is `src/main.rs`; see `installer/README.md` for how the MSI uses it.

pub mod args;
pub mod cleanup;
pub mod guids;
pub mod log;
pub mod pe;
pub mod profile;

#[cfg(windows)]
pub mod sys;

#[cfg(not(windows))]
pub mod sys {
    //! Non-Windows stub so the pure modules build and test anywhere.
    pub fn local_time() -> [u16; 6] {
        [1970, 1, 1, 0, 0, 0]
    }
}

/// Process exit codes (documented in installer/README.md and `/help`).
pub mod exit {
    pub const OK: i32 = 0;
    pub const OTHER: i32 = 1;
    pub const USAGE: i32 = 2;
    pub const BAD_DLL: i32 = 3;
    pub const LOAD_FAILED: i32 = 4;
    pub const MISSING_EXPORT: i32 = 5;
    pub const DLL_CALL_FAILED: i32 = 6;
    pub const ILOT_FAILED: i32 = 7;
    pub const CLEANUP_INCOMPLETE: i32 = 8;
    pub const NOT_REGISTERED: i32 = 10;
}
