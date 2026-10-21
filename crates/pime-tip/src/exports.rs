//! DLL exports, the panic guard and object counting.
//!
//! Every COM entry point (these exports and every `*_Impl` method) runs inside [`guard`]:
//! windows-rs's vtable shims do not catch panics, and a panic unwinding out of an
//! `extern "system"` fn aborts the host process. After a caught panic the service goes inert
//! (`POISONED`) for the rest of the process lifetime.

use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Once;

use windows::Win32::Foundation::{CLASS_E_CLASSNOTAVAILABLE, E_FAIL, E_POINTER, S_FALSE, S_OK};
use windows::Win32::System::Com::IClassFactory;
use windows_core::{Interface, GUID, HRESULT};

use crate::factory::ClassFactory;
use crate::registry::CLSID_TEXT_SERVICE;

/// Set once a panic has been caught; the service then stops eating keys.
pub(crate) static POISONED: AtomicBool = AtomicBool::new(false);
static OBJECTS: AtomicIsize = AtomicIsize::new(0);
static LOCKS: AtomicIsize = AtomicIsize::new(0);
static HOOK: Once = Once::new();

/// Counts live COM objects for `DllCanUnloadNow`. Embed one in every `#[implement]` struct
/// (pime-candidate-ui counts its own UIA providers: `pime_candidate_ui::live_com_objects`).
pub(crate) struct ObjToken(());
impl ObjToken {
    pub(crate) fn new() -> Self {
        OBJECTS.fetch_add(1, Ordering::SeqCst);
        ObjToken(())
    }
}
impl Drop for ObjToken {
    fn drop(&mut self) {
        OBJECTS.fetch_sub(1, Ordering::SeqCst);
    }
}

pub(crate) fn lock_server(lock: bool) {
    LOCKS.fetch_add(if lock { 1 } else { -1 }, Ordering::SeqCst);
}

/// The default hook writes to stderr (useless in a GUI host, and it must never risk printing
/// typed content). Installed lazily: no work in DllMain.
fn install_quiet_hook() {
    HOOK.call_once(|| std::panic::set_hook(Box::new(|_| {})));
}

pub(crate) fn guard<T>(f: impl FnOnce() -> windows_core::Result<T>) -> windows_core::Result<T> {
    install_quiet_hook();
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(_) => {
            POISONED.store(true, Ordering::SeqCst);
            Err(E_FAIL.into())
        }
    }
}

/// For callbacks that cannot report an HRESULT (window procedures, presenter callbacks).
pub(crate) fn guard_unit(f: impl FnOnce()) {
    let _ = guard(|| {
        f();
        Ok(())
    });
}

fn guard_hr(f: impl FnOnce() -> HRESULT) -> HRESULT {
    install_quiet_hook();
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| {
        POISONED.store(true, Ordering::SeqCst);
        E_FAIL
    })
}

pub(crate) fn poisoned() -> bool {
    POISONED.load(Ordering::Relaxed)
}

/// # Safety
/// COM contract: `rclsid`/`riid` point to GUIDs, `ppv` is writable.
#[no_mangle]
pub unsafe extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    guard_hr(|| unsafe {
        if ppv.is_null() || rclsid.is_null() || riid.is_null() {
            return E_POINTER;
        }
        *ppv = std::ptr::null_mut();
        if *rclsid != CLSID_TEXT_SERVICE {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        let factory: IClassFactory = ClassFactory::new().into();
        factory.query(riid, ppv)
    })
}

#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    guard_hr(|| {
        // A pending config-watch thread-pool callback into an unloaded DLL would crash the host,
        // and so would a late Release on a UIA provider a screen reader still holds.
        if OBJECTS.load(Ordering::SeqCst) == 0
            && LOCKS.load(Ordering::SeqCst) == 0
            && pime_candidate_ui::live_com_objects() == 0
            && !crate::reload::is_running()
            && !crate::autocomplete::is_running()
        {
            S_OK
        } else {
            S_FALSE
        }
    })
}

#[no_mangle]
pub extern "system" fn DllRegisterServer() -> HRESULT {
    guard_hr(|| match crate::registry::register() {
        Ok(()) => S_OK,
        Err(e) => {
            // Leave nothing half-registered.
            let _ = crate::registry::unregister();
            e.code()
        }
    })
}

#[no_mangle]
pub extern "system" fn DllUnregisterServer() -> HRESULT {
    guard_hr(|| match crate::registry::unregister() {
        Ok(()) => S_OK,
        Err(e) => e.code(),
    })
}
