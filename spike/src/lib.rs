//! WS1 spike: minimal in-process TSF text service in Rust (windows-rs). Disposable.
//!
//! Safety model: every COM entry point (DLL exports and every `*_Impl` method) runs inside
//! [`guard`], which catches panics and returns `E_FAIL`. windows-rs's generated vtable shims do
//! NOT catch panics, and since Rust 1.81 a panic unwinding out of an `extern "system"` fn aborts
//! the host process, so the guard is mandatory, not defensive. After any caught panic the
//! service goes inert (`POISONED`) for the rest of the process lifetime.

pub mod engine;

#[cfg(windows)]
mod display;
#[cfg(windows)]
mod edit;
#[cfg(windows)]
mod factory;
#[cfg(windows)]
mod guids;
#[cfg(windows)]
mod registry;
#[cfg(windows)]
mod service;

#[cfg(windows)]
pub use exports::*;

#[cfg(windows)]
mod exports {
    use std::ffi::c_void;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
    use std::sync::Once;

    use windows::Win32::Foundation::{CLASS_E_CLASSNOTAVAILABLE, E_FAIL, E_POINTER, S_FALSE, S_OK};
    use windows::Win32::System::Com::IClassFactory;
    use windows_core::{Interface, GUID, HRESULT};

    use crate::factory::ClassFactory;
    use crate::guids::CLSID_TEXT_SERVICE;

    /// Set once a panic has been caught; the service then stops eating keys.
    pub(crate) static POISONED: AtomicBool = AtomicBool::new(false);
    static OBJECTS: AtomicIsize = AtomicIsize::new(0);
    static LOCKS: AtomicIsize = AtomicIsize::new(0);
    static HOOK: Once = Once::new();

    /// Counts live COM objects for `DllCanUnloadNow`. Embed one in every `#[implement]` struct.
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

    /// The default panic hook writes to stderr, which in a GUI host is at best useless; it also
    /// must never risk printing typed content. Installed lazily (no work in DllMain).
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
            if OBJECTS.load(Ordering::SeqCst) == 0 && LOCKS.load(Ordering::SeqCst) == 0 {
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
}

/// In-process COM tests: they create our objects directly and QueryInterface them. Nothing is
/// registered and no TSF manager is involved, so they are safe on the dev machine.
#[cfg(all(test, windows))]
mod com_tests {
    use std::ffi::c_void;

    use windows::Win32::Foundation::{CLASS_E_CLASSNOTAVAILABLE, E_FAIL, S_FALSE};
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::System::Com::IClassFactory;
    use windows::Win32::UI::TextServices::{
        ITfCompositionSink, ITfDisplayAttributeInfo, ITfDisplayAttributeProvider, ITfKeyEventSink,
        ITfTextInputProcessor, ITfTextInputProcessorEx, ITfThreadMgrEventSink, TF_DISPLAYATTRIBUTE,
        TF_LS_SOLID,
    };
    use windows_core::{IUnknown, Interface, GUID};

    use crate::exports::{guard, DllCanUnloadNow, DllGetClassObject, POISONED};
    use crate::guids::{CLSID_TEXT_SERVICE, GUID_DISPLAY_ATTR_INPUT};

    /// POISONED is process-global; tests that set it or depend on it must not overlap.
    static POISON_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn factory() -> IClassFactory {
        let mut p: *mut c_void = std::ptr::null_mut();
        let hr = unsafe { DllGetClassObject(&CLSID_TEXT_SERVICE, &IClassFactory::IID, &mut p) };
        hr.unwrap();
        unsafe { IClassFactory::from_raw(p) }
    }

    #[test]
    fn wrong_clsid_rejected() {
        let mut p: *mut c_void = std::ptr::null_mut();
        let bogus = GUID::from_u128(1);
        let hr = unsafe { DllGetClassObject(&bogus, &IClassFactory::IID, &mut p) };
        assert_eq!(hr, CLASS_E_CLASSNOTAVAILABLE);
        assert!(p.is_null());
    }

    #[test]
    fn one_object_answers_every_tsf_interface() {
        let tip: ITfTextInputProcessorEx = unsafe { factory().CreateInstance(None) }.unwrap();
        tip.cast::<ITfTextInputProcessor>().unwrap();
        tip.cast::<ITfKeyEventSink>().unwrap();
        tip.cast::<ITfThreadMgrEventSink>().unwrap();
        tip.cast::<ITfCompositionSink>().unwrap();
        tip.cast::<ITfDisplayAttributeProvider>().unwrap();
        // COM identity: every interface QIs back to the same IUnknown.
        let a: IUnknown = tip.cast().unwrap();
        let b: IUnknown = tip.cast::<ITfKeyEventSink>().unwrap().cast().unwrap();
        assert_eq!(a.as_raw(), b.as_raw());
        assert_eq!(DllCanUnloadNow(), S_FALSE); // objects alive
    }

    #[test]
    fn display_provider_created_standalone() {
        // TSF CoCreates the CLSID asking directly for ITfDisplayAttributeProvider.
        let p: ITfDisplayAttributeProvider = unsafe { factory().CreateInstance(None) }.unwrap();
        let info: ITfDisplayAttributeInfo =
            unsafe { p.GetDisplayAttributeInfo(&GUID_DISPLAY_ATTR_INPUT) }.unwrap();
        let mut da = TF_DISPLAYATTRIBUTE::default();
        unsafe { info.GetAttributeInfo(&mut da) }.unwrap();
        assert_eq!(da.lsStyle, TF_LS_SOLID);
        assert!(unsafe { p.GetDisplayAttributeInfo(&GUID::from_u128(7)) }.is_err());

        let e = unsafe { p.EnumDisplayAttributeInfo() }.unwrap();
        // The windows-rs caller wrapper maps S_FALSE to Ok(()), so call the raw vtable slot to
        // see the HRESULT TSF actually receives.
        let mut out: [Option<ITfDisplayAttributeInfo>; 2] = [None, None];
        let mut n = 0u32;
        let next = Interface::vtable(&e).Next;
        let hr = unsafe { next(e.as_raw(), 2, out.as_mut_ptr().cast(), &mut n) };
        assert_eq!(
            (hr, n),
            (S_FALSE, 1),
            "enumerator must report S_FALSE at the end"
        );
        assert!(out[0].is_some());
        let mut n2 = 9u32;
        let hr = unsafe { next(e.as_raw(), 1, out.as_mut_ptr().cast(), &mut n2) };
        assert_eq!((hr, n2), (S_FALSE, 0));
    }

    #[test]
    fn null_arguments_fail_cleanly() {
        let _g = POISON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tip: ITfTextInputProcessorEx = unsafe { factory().CreateInstance(None) }.unwrap();
        // Null thread manager: an error HRESULT, not a crash.
        assert!(unsafe { tip.ActivateEx(None, 1, 0) }.is_err());
        let keys: ITfKeyEventSink = tip.cast().unwrap();
        let eaten = unsafe { keys.OnTestKeyDown(None, WPARAM(0x41), LPARAM(0)) }.unwrap();
        assert!(!eaten.as_bool());
        unsafe { tip.Deactivate() }.unwrap();
    }

    #[test]
    fn guard_turns_panic_into_e_fail() {
        let _g = POISON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let r: windows_core::Result<()> = guard(|| panic!("boom"));
        assert_eq!(r.unwrap_err().code(), E_FAIL);
        assert!(POISONED.swap(false, std::sync::atomic::Ordering::SeqCst));
    }
}
