//! COM + TSF (un)registration and small registry reads. Registration runs only inside
//! `DllRegisterServer` / `DllUnregisterServer` (regsvr32 / pime-setup in a VM); never on the key
//! path.

use windows::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, HMODULE, RPC_E_CHANGED_MODE, WIN32_ERROR,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::LibraryLoader::{
    GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
    GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyW, RegDeleteTreeW, RegGetValueW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
    RRF_RT_REG_DWORD,
};
use windows::Win32::UI::Input::KeyboardAndMouse::HKL;
use windows::Win32::UI::TextServices::{
    CLSID_TF_CategoryMgr, CLSID_TF_InputProcessorProfiles, ITfCategoryMgr,
    ITfInputProcessorProfileMgr, ITfInputProcessorProfiles, GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER,
    GUID_TFCAT_TIPCAP_COMLESS, GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT,
    GUID_TFCAT_TIPCAP_INPUTMODECOMPARTMENT, GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT,
    GUID_TFCAT_TIPCAP_UIELEMENTENABLED, GUID_TFCAT_TIP_KEYBOARD,
};
use windows_core::{Interface, GUID, HRESULT, PCWSTR};

pub const CLSID_TEXT_SERVICE: GUID = GUID::from_u128(pime_ids::CLSID_TEXT_SERVICE);
pub const GUID_PROFILE: GUID = GUID::from_u128(pime_ids::GUID_PROFILE_FA_PHONETIC);
pub const GUID_DISPLAY_ATTR_INPUT: GUID = GUID::from_u128(pime_ids::GUID_DISPLAY_ATTR_INPUT);
pub const GUID_DISPLAY_ATTR_PENDING: GUID = GUID::from_u128(pime_ids::GUID_DISPLAY_ATTR_PENDING);
pub const GUID_DISPLAY_ATTR_SELECTED: GUID = GUID::from_u128(pime_ids::GUID_DISPLAY_ATTR_SELECTED);

/// PLAN 5.4. No TIPCAP_SECUREMODE in v1.
const CATEGORIES: &[GUID] = &[
    GUID_TFCAT_TIP_KEYBOARD,
    GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER,
    GUID_TFCAT_TIPCAP_UIELEMENTENABLED,
    GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT,
    GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT,
    GUID_TFCAT_TIPCAP_INPUTMODECOMPARTMENT,
    GUID_TFCAT_TIPCAP_COMLESS,
];

pub const APP_KEY: &str = r"Software\PersianPhoneticIme";
/// Text-service runtime switches (see `settings.rs`).
pub const TIP_KEY: &str = r"Software\PersianPhoneticIme\Tip";
const CLSID_DESCRIPTION: &str = "Persian Phonetic IME text service";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub(crate) fn clsid_key() -> String {
    format!(
        r"Software\Classes\CLSID\{}",
        pime_ids::braced(pime_ids::CLSID_TEXT_SERVICE)
    )
}

/// Full path of this DLL, found from an address inside it (no DllMain needed).
pub(crate) fn dll_path() -> windows_core::Result<String> {
    let mut module = HMODULE::default();
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(dll_path as *const () as *const u16),
            &mut module,
        )?;
    }
    let mut buf = vec![0u16; 260];
    loop {
        let n = unsafe { GetModuleFileNameW(Some(module), &mut buf) } as usize;
        if n == 0 {
            return Err(windows_core::Error::from_thread());
        }
        if n < buf.len() {
            return Ok(String::from_utf16_lossy(&buf[..n]));
        }
        buf.resize(buf.len() * 2, 0);
    }
}

fn check(err: WIN32_ERROR) -> windows_core::Result<()> {
    if err == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(HRESULT::from_win32(err.0).into())
    }
}

fn set_string(
    root: HKEY,
    subkey: &str,
    name: Option<&str>,
    value: &str,
) -> windows_core::Result<()> {
    let mut key = HKEY::default();
    let sub = wide(subkey);
    unsafe {
        check(RegCreateKeyExW(
            root,
            PCWSTR(sub.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut key,
            None,
        ))?;
        let data: Vec<u8> = wide(value).iter().flat_map(|u| u.to_le_bytes()).collect();
        let name_w = name.map(wide);
        let name_p = name_w
            .as_ref()
            .map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr()));
        let r = check(RegSetValueExW(key, name_p, None, REG_SZ, Some(&data)));
        let _ = RegCloseKey(key);
        r
    }
}

pub fn read_dword(root: HKEY, subkey: &str, name: &str) -> Option<u32> {
    let sub = wide(subkey);
    let name = wide(name);
    let mut value = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let err = unsafe {
        RegGetValueW(
            root,
            PCWSTR(sub.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    (err == ERROR_SUCCESS).then_some(value)
}

/// Kill switch (PLAN 0): `Disabled` != 0 under HKCU or HKLM `Software\PersianPhoneticIme`.
pub fn kill_switch() -> bool {
    [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE]
        .into_iter()
        .any(|root| read_dword(root, APP_KEY, "Disabled").unwrap_or(0) != 0)
}

/// regsvr32 initialises COM itself, but other callers (installer helpers) may not.
struct ComInit(bool);
impl ComInit {
    fn new() -> windows_core::Result<Self> {
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if hr == RPC_E_CHANGED_MODE {
            return Ok(ComInit(false));
        }
        hr.ok()?;
        Ok(ComInit(true))
    }
}
impl Drop for ComInit {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

/// `PIME_HKL_SUBSTITUTE=1` in the registering process's environment registers the profile with
/// hklSubstitute 0x04090409 (PLAN 2.4 experiment; sandbox only, `register.ps1 -HklSubstitute`).
fn hkl_substitute() -> HKL {
    if std::env::var_os("PIME_HKL_SUBSTITUTE").is_some_and(|v| v == "1") {
        HKL(0x0409_0409usize as *mut _)
    } else {
        HKL::default()
    }
}

pub fn register() -> windows_core::Result<()> {
    let _com = ComInit::new()?;
    let path = dll_path()?;

    // 1. COM server. HKLM\Software\Classes is redirected to WOW6432Node for the 32-bit DLL, so
    //    x64 and x86 share one CLSID. Re-registering simply repoints InprocServer32 (upgrade).
    set_string(HKEY_LOCAL_MACHINE, &clsid_key(), None, CLSID_DESCRIPTION)?;
    let inproc = format!(r"{}\InprocServer32", clsid_key());
    set_string(HKEY_LOCAL_MACHINE, &inproc, None, &path)?;
    set_string(
        HKEY_LOCAL_MACHINE,
        &inproc,
        Some("ThreadingModel"),
        "Apartment",
    )?;

    // 2. TSF profile.
    let desc: Vec<u16> = pime_ids::PROFILE_DESCRIPTION.encode_utf16().collect();
    let icon: Vec<u16> = path.encode_utf16().collect();
    unsafe {
        let profiles: ITfInputProcessorProfileMgr =
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?;
        profiles.RegisterProfile(
            &CLSID_TEXT_SERVICE,
            pime_ids::LANGID_FA_IR,
            &GUID_PROFILE,
            &desc,
            &icon,
            0, // no icon resource yet: Windows falls back to the language tag
            hkl_substitute(),
            0,
            true,
            0,
        )?;

        // 3. Categories (PLAN 5.4).
        let cats: ITfCategoryMgr =
            CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)?;
        for cat in CATEGORIES {
            cats.RegisterCategory(&CLSID_TEXT_SERVICE, cat, &CLSID_TEXT_SERVICE)?;
        }
    }
    Ok(())
}

/// Reverse of `register`, best-effort: every step runs even if an earlier one failed, and
/// "already gone" is success, so unregister is idempotent and safe after a partial register.
pub fn unregister() -> windows_core::Result<()> {
    let _com = ComInit::new()?;
    let mut first_err: Option<windows_core::Error> = None;
    let mut note = |r: windows_core::Result<()>| {
        if let Err(e) = r {
            first_err.get_or_insert(e);
        }
    };

    unsafe {
        match CoCreateInstance::<_, ITfCategoryMgr>(
            &CLSID_TF_CategoryMgr,
            None,
            CLSCTX_INPROC_SERVER,
        ) {
            Ok(cats) => {
                for cat in CATEGORIES {
                    let _ = cats.UnregisterCategory(&CLSID_TEXT_SERVICE, cat, &CLSID_TEXT_SERVICE);
                }
            }
            Err(e) => note(Err(e)),
        }
        match CoCreateInstance::<_, ITfInputProcessorProfileMgr>(
            &CLSID_TF_InputProcessorProfiles,
            None,
            CLSCTX_INPROC_SERVER,
        ) {
            Ok(mgr) => {
                let _ = mgr.UnregisterProfile(
                    &CLSID_TEXT_SERVICE,
                    pime_ids::LANGID_FA_IR,
                    &GUID_PROFILE,
                    0,
                );
                // Sweeps the whole HKLM\Software\Microsoft\CTF\TIP\{clsid} key.
                if let Ok(old) = mgr.cast::<ITfInputProcessorProfiles>() {
                    let _ = old.Unregister(&CLSID_TEXT_SERVICE);
                }
            }
            Err(e) => note(Err(e)),
        }
        let key = wide(&clsid_key());
        let err = RegDeleteTreeW(HKEY_LOCAL_MACHINE, PCWSTR(key.as_ptr()));
        if err != ERROR_FILE_NOT_FOUND {
            note(check(err));
            let _ = RegDeleteKeyW(HKEY_LOCAL_MACHINE, PCWSTR(key.as_ptr()));
        }
    }
    first_err.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clsid_key_is_braced_registry_form() {
        assert_eq!(
            clsid_key(),
            r"Software\Classes\CLSID\{49B3A09B-D707-4675-B4EB-62B17CCB6D73}"
        );
        assert_eq!(
            format!("{{{:?}}}", CLSID_TEXT_SERVICE),
            pime_ids::braced(pime_ids::CLSID_TEXT_SERVICE)
        );
    }

    #[test]
    fn product_guids_not_spike() {
        assert_ne!(
            CLSID_TEXT_SERVICE,
            GUID::from_u128(0x674850a2_4c3b_46fa_9b17_3ca5465ffa18)
        );
        assert_eq!(pime_ids::PROFILE_DESCRIPTION, "Persian Phonetic");
    }

    #[test]
    fn dll_path_resolves_to_this_module() {
        let p = dll_path().unwrap().to_ascii_lowercase();
        assert!(p.ends_with(".exe") || p.ends_with(".dll"));
    }

    #[test]
    fn kill_switch_read_is_harmless() {
        // Read-only; whatever the machine says, it must not panic.
        let _ = kill_switch();
    }
}
