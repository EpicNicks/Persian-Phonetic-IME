//! Display attribute (underline) for the composition.
use std::cell::Cell;

use windows::Win32::Foundation::{E_INVALIDARG, E_POINTER, S_FALSE};
use windows::Win32::UI::TextServices::{
    IEnumTfDisplayAttributeInfo, IEnumTfDisplayAttributeInfo_Impl, ITfDisplayAttributeInfo,
    ITfDisplayAttributeInfo_Impl, TF_ATTR_INPUT, TF_CT_NONE, TF_DA_COLOR, TF_DISPLAYATTRIBUTE,
    TF_LS_SOLID,
};
use windows_core::{implement, Error, BSTR, GUID};

use crate::exports::{guard, ObjToken};
use crate::guids::GUID_DISPLAY_ATTR_INPUT;

fn input_attribute() -> TF_DISPLAYATTRIBUTE {
    let none = TF_DA_COLOR {
        r#type: TF_CT_NONE,
        ..Default::default()
    };
    TF_DISPLAYATTRIBUTE {
        crText: none,
        crBk: none,
        lsStyle: TF_LS_SOLID,
        fBoldLine: false.into(),
        crLine: none,
        bAttr: TF_ATTR_INPUT,
    }
}

pub fn info_for(guid: &GUID) -> windows_core::Result<ITfDisplayAttributeInfo> {
    if *guid == GUID_DISPLAY_ATTR_INPUT {
        Ok(DisplayAttributeInfo::new().into())
    } else {
        Err(E_INVALIDARG.into())
    }
}

#[implement(ITfDisplayAttributeInfo)]
pub struct DisplayAttributeInfo {
    attr: Cell<TF_DISPLAYATTRIBUTE>,
    _tok: ObjToken,
}

impl DisplayAttributeInfo {
    fn new() -> Self {
        DisplayAttributeInfo {
            attr: Cell::new(input_attribute()),
            _tok: ObjToken::new(),
        }
    }
}

impl ITfDisplayAttributeInfo_Impl for DisplayAttributeInfo_Impl {
    fn GetGUID(&self) -> windows_core::Result<GUID> {
        guard(|| Ok(GUID_DISPLAY_ATTR_INPUT))
    }
    fn GetDescription(&self) -> windows_core::Result<BSTR> {
        guard(|| Ok(BSTR::from("Composing")))
    }
    fn GetAttributeInfo(&self, pda: *mut TF_DISPLAYATTRIBUTE) -> windows_core::Result<()> {
        guard(|| {
            if pda.is_null() {
                return Err(E_POINTER.into());
            }
            unsafe { pda.write(self.attr.get()) };
            Ok(())
        })
    }
    fn SetAttributeInfo(&self, pda: *const TF_DISPLAYATTRIBUTE) -> windows_core::Result<()> {
        guard(|| {
            if pda.is_null() {
                return Err(E_POINTER.into());
            }
            self.attr.set(unsafe { pda.read() });
            Ok(())
        })
    }
    fn Reset(&self) -> windows_core::Result<()> {
        guard(|| {
            self.attr.set(input_attribute());
            Ok(())
        })
    }
}

const ATTRIBUTES: &[GUID] = &[GUID_DISPLAY_ATTR_INPUT];

#[implement(IEnumTfDisplayAttributeInfo)]
pub struct EnumDisplayAttributeInfo {
    index: Cell<usize>,
    _tok: ObjToken,
}

impl EnumDisplayAttributeInfo {
    pub fn new(index: usize) -> Self {
        EnumDisplayAttributeInfo {
            index: Cell::new(index),
            _tok: ObjToken::new(),
        }
    }
}

impl IEnumTfDisplayAttributeInfo_Impl for EnumDisplayAttributeInfo_Impl {
    fn Clone(&self) -> windows_core::Result<IEnumTfDisplayAttributeInfo> {
        guard(|| Ok(EnumDisplayAttributeInfo::new(self.index.get()).into()))
    }

    fn Next(
        &self,
        count: u32,
        out: *mut Option<ITfDisplayAttributeInfo>,
        fetched: *mut u32,
    ) -> windows_core::Result<()> {
        guard(|| {
            if out.is_null() {
                return Err(E_POINTER.into());
            }
            let mut n = 0u32;
            while n < count && self.index.get() < ATTRIBUTES.len() {
                let info = info_for(&ATTRIBUTES[self.index.get()])?;
                unsafe { out.add(n as usize).write(Some(info)) };
                n += 1;
                self.index.set(self.index.get() + 1);
            }
            if !fetched.is_null() {
                unsafe { fetched.write(n) };
            }
            // The trait returns Result<()>, so the only way to hand back the S_FALSE an
            // enumerator owes its caller at the end is to smuggle it through Err.
            if n < count {
                Err(Error::from_hresult(S_FALSE))
            } else {
                Ok(())
            }
        })
    }

    fn Reset(&self) -> windows_core::Result<()> {
        guard(|| {
            self.index.set(0);
            Ok(())
        })
    }

    fn Skip(&self, count: u32) -> windows_core::Result<()> {
        guard(|| {
            let target = self.index.get() + count as usize;
            self.index.set(target.min(ATTRIBUTES.len()));
            if target > ATTRIBUTES.len() {
                Err(Error::from_hresult(S_FALSE))
            } else {
                Ok(())
            }
        })
    }
}
