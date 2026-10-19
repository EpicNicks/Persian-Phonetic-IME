use std::ffi::c_void;

use windows::Win32::Foundation::{CLASS_E_NOAGGREGATION, E_POINTER};
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows_core::{implement, IUnknown, Interface, Ref, BOOL, GUID};

use crate::exports::{guard, lock_server, ObjToken};
use crate::service::TextService;

#[implement(IClassFactory)]
pub struct ClassFactory {
    _tok: ObjToken,
}

impl ClassFactory {
    pub fn new() -> Self {
        ClassFactory {
            _tok: ObjToken::new(),
        }
    }
}

impl IClassFactory_Impl for ClassFactory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<IUnknown>,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> windows_core::Result<()> {
        guard(|| unsafe {
            if ppv.is_null() || riid.is_null() {
                return Err(E_POINTER.into());
            }
            *ppv = std::ptr::null_mut();
            if !outer.is_null() {
                return Err(CLASS_E_NOAGGREGATION.into());
            }
            // TSF asks for ITfTextInputProcessor(Ex) on activation, and separately CoCreates us
            // for ITfDisplayAttributeProvider; plain QI covers both. `new` does no work.
            let unk: IUnknown = TextService::new().into();
            unk.query(riid, ppv).ok()
        })
    }

    fn LockServer(&self, lock: BOOL) -> windows_core::Result<()> {
        guard(|| {
            lock_server(lock.as_bool());
            Ok(())
        })
    }
}
