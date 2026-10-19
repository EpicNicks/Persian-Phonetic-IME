//! Thin Win32 wrappers. Everything that changes system state is called only from the
//! `/register`, `/unregister`, `/enable-user`, `/disable-user` and `/cleanup-old` commands;
//! the registry helpers here are read-only (KEY_READ).

use std::ffi::{c_void, CStr, OsStr};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{
    FreeLibrary, GetLastError, ERROR_MORE_DATA, ERROR_SUCCESS, FILETIME, SYSTEMTIME,
};
use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_DELAY_UNTIL_REBOOT};
use windows_sys::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows_sys::Win32::System::Console::{GetConsoleProcessList, GetConsoleWindow};
use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32, LOAD_WITH_ALTERED_SEARCH_PATH,
};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegEnumValueW, RegGetValueW, RegOpenKeyExW, HKEY,
    HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
    RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};

pub fn local_time() -> [u16; 6] {
    // SAFETY: GetLocalTime only writes the struct we pass.
    let t = unsafe {
        let mut t: SYSTEMTIME = std::mem::zeroed();
        GetLocalTime(&mut t);
        t
    };
    [t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond]
}

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}

fn last_error() -> u32 {
    // SAFETY: trivial.
    unsafe { GetLastError() }
}

/// Hide the console window if this process owns it alone (Active Setup at logon), so users do
/// not see a black box flash for long.
pub fn hide_own_console() {
    // SAFETY: plain calls with valid buffers.
    unsafe {
        let mut pids = [0u32; 2];
        if GetConsoleProcessList(pids.as_mut_ptr(), 2) == 1 {
            let w = GetConsoleWindow();
            if !w.is_null() {
                ShowWindow(w, SW_HIDE);
            }
        }
    }
}

/// Apartment-threaded COM for the duration of a DllRegisterServer call (regsvr32 does the same).
pub struct Com(bool);

impl Com {
    pub fn init() -> Com {
        // SAFETY: reserved pointer must be null.
        let hr = unsafe { CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32) };
        Com(hr >= 0)
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: balanced with a successful CoInitializeEx on this thread.
            unsafe { CoUninitialize() };
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallError {
    LoadFailed(u32),
    MissingExport,
}

/// Loads `dll` (absolute path) and calls `export`, an `HRESULT WINAPI f(void)`.
pub fn call_hresult_export(dll: &Path, export: &CStr) -> Result<i32, CallError> {
    let path = wide(dll.as_os_str());
    // SAFETY: valid NUL-terminated strings; the function pointer type matches the documented
    // signature of DllRegisterServer/DllUnregisterServer (STDAPI, no arguments).
    unsafe {
        let h = LoadLibraryExW(path.as_ptr(), null_mut(), LOAD_WITH_ALTERED_SEARCH_PATH);
        if h.is_null() {
            return Err(CallError::LoadFailed(last_error()));
        }
        let result = match GetProcAddress(h, export.as_ptr().cast()) {
            Some(f) => {
                let f: unsafe extern "system" fn() -> i32 = std::mem::transmute(f);
                Ok(f())
            }
            None => Err(CallError::MissingExport),
        };
        FreeLibrary(h);
        result
    }
}

/// `BOOL InstallLayoutOrTip(LPCWSTR psz, DWORD dwFlags)` from System32\input.dll.
pub fn install_layout_or_tip(psz: &str, flags: u32) -> Result<bool, CallError> {
    let dll = wide(OsStr::new("input.dll"));
    let arg = wide(OsStr::new(psz));
    // SAFETY: input.dll is loaded from System32 only; signature per docs/spike.md 4.7.
    unsafe {
        let h = LoadLibraryExW(dll.as_ptr(), null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32);
        if h.is_null() {
            return Err(CallError::LoadFailed(last_error()));
        }
        let result = match GetProcAddress(h, c"InstallLayoutOrTip".as_ptr().cast()) {
            Some(f) => {
                let f: unsafe extern "system" fn(*const u16, u32) -> i32 = std::mem::transmute(f);
                Ok(f(arg.as_ptr(), flags) != 0)
            }
            None => Err(CallError::MissingExport),
        };
        FreeLibrary(h);
        result
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Root {
    LocalMachine,
    CurrentUser,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Native,
    Bits64,
    Bits32,
}

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: opened by RegOpenKeyExW.
        unsafe { RegCloseKey(self.0) };
    }
}

fn open(root: Root, view: View, subkey: &str) -> Option<Key> {
    let root = match root {
        Root::LocalMachine => HKEY_LOCAL_MACHINE,
        Root::CurrentUser => HKEY_CURRENT_USER,
    };
    let sam = KEY_READ
        | match view {
            View::Native => 0,
            View::Bits64 => KEY_WOW64_64KEY,
            View::Bits32 => KEY_WOW64_32KEY,
        };
    let sub = wide(OsStr::new(subkey));
    let mut h: HKEY = null_mut();
    // SAFETY: read-only open; out pointer valid.
    let rc = unsafe { RegOpenKeyExW(root, sub.as_ptr(), 0, sam, &mut h) };
    (rc == ERROR_SUCCESS).then_some(Key(h))
}

pub fn key_exists(root: Root, view: View, subkey: &str) -> bool {
    open(root, view, subkey).is_some()
}

/// REG_SZ / REG_EXPAND_SZ (expanded). `value = None` reads the default value.
pub fn read_string(root: Root, view: View, subkey: &str, value: Option<&str>) -> Option<String> {
    let key = open(root, view, subkey)?;
    let name = value.map(|v| wide(OsStr::new(v)));
    let name_ptr = name.as_ref().map_or(null(), |n| n.as_ptr());
    let mut bytes = 0u32;
    // SAFETY: size query, then a read into a buffer of that size.
    unsafe {
        let rc = RegGetValueW(
            key.0,
            null(),
            name_ptr,
            RRF_RT_REG_SZ,
            null_mut(),
            null_mut(),
            &mut bytes,
        );
        if rc != ERROR_SUCCESS {
            return None;
        }
        let mut buf = vec![0u16; (bytes as usize).div_ceil(2) + 1];
        bytes = (buf.len() * 2) as u32;
        let rc = RegGetValueW(
            key.0,
            null(),
            name_ptr,
            RRF_RT_REG_SZ,
            null_mut(),
            buf.as_mut_ptr().cast::<c_void>(),
            &mut bytes,
        );
        if rc != ERROR_SUCCESS {
            return None;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    }
}

pub fn read_dword(root: Root, view: View, subkey: &str, value: &str) -> Option<u32> {
    let key = open(root, view, subkey)?;
    let name = wide(OsStr::new(value));
    let mut data = 0u32;
    let mut bytes = 4u32;
    // SAFETY: 4-byte buffer for a DWORD.
    let rc = unsafe {
        RegGetValueW(
            key.0,
            null(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            (&mut data as *mut u32).cast::<c_void>(),
            &mut bytes,
        )
    };
    (rc == ERROR_SUCCESS).then_some(data)
}

pub fn subkeys(root: Root, view: View, subkey: &str) -> Vec<String> {
    let Some(key) = open(root, view, subkey) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for i in 0.. {
        let mut buf = [0u16; 256];
        let mut len = buf.len() as u32;
        // SAFETY: buffer and length match; optional out params are null.
        let rc = unsafe {
            RegEnumKeyExW(
                key.0,
                i,
                buf.as_mut_ptr(),
                &mut len,
                null(),
                null_mut(),
                null_mut(),
                null_mut::<FILETIME>(),
            )
        };
        if rc != ERROR_SUCCESS {
            break;
        }
        out.push(String::from_utf16_lossy(&buf[..len as usize]));
    }
    out
}

pub fn value_names(root: Root, view: View, subkey: &str) -> Vec<String> {
    let Some(key) = open(root, view, subkey) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for i in 0.. {
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        // SAFETY: names only; data pointers null.
        let rc = unsafe {
            RegEnumValueW(
                key.0,
                i,
                buf.as_mut_ptr(),
                &mut len,
                null(),
                null_mut(),
                null_mut(),
                null_mut(),
            )
        };
        if rc == ERROR_MORE_DATA {
            continue; // a name longer than ours can be skipped
        }
        if rc != ERROR_SUCCESS {
            break;
        }
        out.push(String::from_utf16_lossy(&buf[..len as usize]));
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposal {
    Deleted,
    AtReboot,
    Failed(u32),
}

/// Delete a file or empty directory now; if it is in use, schedule it with
/// MoveFileEx(MOVEFILE_DELAY_UNTIL_REBOOT) (needs admin, i.e. the deferred SYSTEM custom action).
pub fn remove_now_or_at_reboot(path: &Path, is_dir: bool) -> Disposal {
    let now = if is_dir {
        std::fs::remove_dir(path)
    } else {
        std::fs::remove_file(path)
    };
    if now.is_ok() {
        return Disposal::Deleted;
    }
    let p = wide(path.as_os_str());
    // SAFETY: valid path; null destination means "delete at reboot".
    if unsafe { MoveFileExW(p.as_ptr(), null(), MOVEFILE_DELAY_UNTIL_REBOOT) } != 0 {
        Disposal::AtReboot
    } else {
        Disposal::Failed(last_error())
    }
}
