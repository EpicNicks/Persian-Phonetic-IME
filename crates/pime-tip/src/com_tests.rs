//! In-process COM tests: they create our objects directly and QueryInterface them. Nothing is
//! registered and no TSF manager is involved, so they are safe on the dev machine.

use std::ffi::c_void;

use windows::Win32::Foundation::{CLASS_E_CLASSNOTAVAILABLE, E_FAIL, LPARAM, S_FALSE, WPARAM};
use windows::Win32::System::Com::IClassFactory;
use windows::Win32::UI::TextServices::{
    ITfCompartmentEventSink, ITfCompositionSink, ITfDisplayAttributeInfo,
    ITfDisplayAttributeProvider, ITfKeyEventSink, ITfTextEditSink, ITfTextInputProcessor,
    ITfTextInputProcessorEx, ITfTextLayoutSink, ITfThreadFocusSink, ITfThreadMgrEventSink,
    TF_ATTR_TARGET_CONVERTED, TF_CT_COLORREF, TF_DISPLAYATTRIBUTE, TF_LS_DOT, TF_LS_SOLID,
};
use windows_core::{IUnknown, Interface, GUID};

use crate::exports::{guard, DllCanUnloadNow, DllGetClassObject, POISONED};
use crate::registry::{
    CLSID_TEXT_SERVICE, GUID_DISPLAY_ATTR_INPUT, GUID_DISPLAY_ATTR_PENDING,
    GUID_DISPLAY_ATTR_SELECTED,
};

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
    tip.cast::<ITfCompartmentEventSink>().unwrap();
    tip.cast::<ITfThreadFocusSink>().unwrap();
    tip.cast::<ITfTextEditSink>().unwrap();
    tip.cast::<ITfTextLayoutSink>().unwrap();
    let a: IUnknown = tip.cast().unwrap();
    let b: IUnknown = tip.cast::<ITfTextLayoutSink>().unwrap().cast().unwrap();
    assert_eq!(a.as_raw(), b.as_raw(), "COM identity");
    assert_eq!(DllCanUnloadNow(), S_FALSE, "objects alive");
}

#[test]
fn display_provider_created_standalone() {
    // TSF CoCreates the CLSID asking directly for ITfDisplayAttributeProvider.
    let p: ITfDisplayAttributeProvider = unsafe { factory().CreateInstance(None) }.unwrap();
    let get = |g: &GUID| {
        let info: ITfDisplayAttributeInfo = unsafe { p.GetDisplayAttributeInfo(g) }.unwrap();
        assert_eq!(unsafe { info.GetGUID() }.unwrap(), *g);
        let mut da = TF_DISPLAYATTRIBUTE::default();
        unsafe { info.GetAttributeInfo(&mut da) }.unwrap();
        da
    };
    assert_eq!(get(&GUID_DISPLAY_ATTR_INPUT).lsStyle, TF_LS_SOLID);
    let pending = get(&GUID_DISPLAY_ATTR_PENDING);
    assert_eq!(pending.lsStyle, TF_LS_DOT);
    assert_eq!(pending.crText.r#type, TF_CT_COLORREF);
    let sel = get(&GUID_DISPLAY_ATTR_SELECTED);
    assert!(sel.fBoldLine.as_bool());
    assert_eq!(sel.bAttr, TF_ATTR_TARGET_CONVERTED);
    assert!(unsafe { p.GetDisplayAttributeInfo(&GUID::from_u128(7)) }.is_err());

    let e = unsafe { p.EnumDisplayAttributeInfo() }.unwrap();
    // The windows-rs caller wrapper maps S_FALSE to Ok(()), so call the raw vtable slot to see
    // the HRESULT TSF actually receives.
    let mut out: [Option<ITfDisplayAttributeInfo>; 4] = [None, None, None, None];
    let mut n = 0u32;
    let next = Interface::vtable(&e).Next;
    let hr = unsafe { next(e.as_raw(), 4, out.as_mut_ptr().cast(), &mut n) };
    assert_eq!((hr, n), (S_FALSE, 3), "S_FALSE at the end");
    assert!(out[..3].iter().all(Option::is_some));
    let mut n2 = 9u32;
    let hr = unsafe { next(e.as_raw(), 1, out.as_mut_ptr().cast(), &mut n2) };
    assert_eq!((hr, n2), (S_FALSE, 0));
}

#[test]
fn inactive_service_never_eats_and_null_args_fail_cleanly() {
    let _g = POISON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tip: ITfTextInputProcessorEx = unsafe { factory().CreateInstance(None) }.unwrap();
    // Null thread manager: an error HRESULT (or the kill switch's S_OK), never a crash.
    let _ = unsafe { tip.ActivateEx(None, 1, 0) };
    let keys: ITfKeyEventSink = tip.cast().unwrap();
    for vk in [0x41usize, 0x20, 0x0D, 0x08] {
        let t = unsafe { keys.OnTestKeyDown(None, WPARAM(vk), LPARAM(1)) }.unwrap();
        let d = unsafe { keys.OnKeyDown(None, WPARAM(vk), LPARAM(1)) }.unwrap();
        let u = unsafe { keys.OnKeyUp(None, WPARAM(vk), LPARAM(1)) }.unwrap();
        assert!(!t.as_bool() && !d.as_bool() && !u.as_bool());
    }
    let comp: ITfCompositionSink = tip.cast().unwrap();
    unsafe { comp.OnCompositionTerminated(0, None) }.unwrap();
    let focus: ITfThreadFocusSink = tip.cast().unwrap();
    unsafe { focus.OnKillThreadFocus() }.unwrap();
    unsafe { focus.OnSetThreadFocus() }.unwrap();
    let cs: ITfCompartmentEventSink = tip.cast().unwrap();
    unsafe { cs.OnChange(std::ptr::null()) }.unwrap();
    unsafe { tip.Deactivate() }.unwrap();
    assert!(!POISONED.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn guard_turns_panic_into_e_fail() {
    let _g = POISON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let r: windows_core::Result<()> = guard(|| panic!("boom"));
    assert_eq!(r.unwrap_err().code(), E_FAIL);
    assert!(POISONED.swap(false, std::sync::atomic::Ordering::SeqCst));
}
