//! Display attributes: input (solid underline), pending (dotted, grey text: a lone `c` waiting
//! for its second letter) and selected (thick underline; reserved, not applied in v1).
use std::cell::Cell;

use windows::Win32::Foundation::{COLORREF, E_INVALIDARG, E_POINTER, S_FALSE};
use windows::Win32::UI::TextServices::{
    IEnumTfDisplayAttributeInfo, IEnumTfDisplayAttributeInfo_Impl, ITfDisplayAttributeInfo,
    ITfDisplayAttributeInfo_Impl, TF_ATTR_INPUT, TF_ATTR_TARGET_CONVERTED, TF_CT_COLORREF,
    TF_CT_NONE, TF_DA_COLOR, TF_DA_COLOR_0, TF_DISPLAYATTRIBUTE, TF_LS_DOT, TF_LS_SOLID,
};
use windows_core::{implement, Error, BSTR, GUID};

use crate::exports::{guard, ObjToken};
use crate::registry::{
    GUID_DISPLAY_ATTR_INPUT, GUID_DISPLAY_ATTR_PENDING, GUID_DISPLAY_ATTR_SELECTED,
};

pub const ATTRIBUTES: &[GUID] = &[
    GUID_DISPLAY_ATTR_INPUT,
    GUID_DISPLAY_ATTR_PENDING,
    GUID_DISPLAY_ATTR_SELECTED,
];

const NONE: TF_DA_COLOR = TF_DA_COLOR {
    r#type: TF_CT_NONE,
    Anonymous: TF_DA_COLOR_0 { nIndex: 0 },
};

fn grey() -> TF_DA_COLOR {
    TF_DA_COLOR {
        r#type: TF_CT_COLORREF,
        Anonymous: TF_DA_COLOR_0 {
            cr: COLORREF(0x0080_8080),
        },
    }
}

pub fn attribute(guid: &GUID) -> Option<(TF_DISPLAYATTRIBUTE, &'static str)> {
    let base = TF_DISPLAYATTRIBUTE {
        crText: NONE,
        crBk: NONE,
        lsStyle: TF_LS_SOLID,
        fBoldLine: false.into(),
        crLine: NONE,
        bAttr: TF_ATTR_INPUT,
    };
    if *guid == GUID_DISPLAY_ATTR_INPUT {
        Some((base, "Composing"))
    } else if *guid == GUID_DISPLAY_ATTR_PENDING {
        Some((
            TF_DISPLAYATTRIBUTE {
                crText: grey(),
                lsStyle: TF_LS_DOT,
                ..base
            },
            "Composing (waiting for the next letter)",
        ))
    } else if *guid == GUID_DISPLAY_ATTR_SELECTED {
        Some((
            TF_DISPLAYATTRIBUTE {
                fBoldLine: true.into(),
                bAttr: TF_ATTR_TARGET_CONVERTED,
                ..base
            },
            "Selected candidate",
        ))
    } else {
        None
    }
}

pub fn info_for(guid: &GUID) -> windows_core::Result<ITfDisplayAttributeInfo> {
    match attribute(guid) {
        Some(_) => Ok(DisplayAttributeInfo::new(*guid).into()),
        None => Err(E_INVALIDARG.into()),
    }
}

#[implement(ITfDisplayAttributeInfo)]
pub struct DisplayAttributeInfo {
    guid: GUID,
    attr: Cell<TF_DISPLAYATTRIBUTE>,
    _tok: ObjToken,
}

impl DisplayAttributeInfo {
    fn new(guid: GUID) -> Self {
        DisplayAttributeInfo {
            guid,
            attr: Cell::new(attribute(&guid).map(|a| a.0).unwrap_or_default()),
            _tok: ObjToken::new(),
        }
    }
}

impl ITfDisplayAttributeInfo_Impl for DisplayAttributeInfo_Impl {
    fn GetGUID(&self) -> windows_core::Result<GUID> {
        guard(|| Ok(self.guid))
    }
    fn GetDescription(&self) -> windows_core::Result<BSTR> {
        guard(|| Ok(BSTR::from(attribute(&self.guid).map_or("", |a| a.1))))
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
            if let Some((a, _)) = attribute(&self.guid) {
                self.attr.set(a);
            }
            Ok(())
        })
    }
}

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
            // The trait returns Result<()>: the S_FALSE an enumerator owes its caller at the end
            // can only be handed back through Err (the shim returns it as-is).
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
