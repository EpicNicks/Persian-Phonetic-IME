//! UI Automation provider: a `List` fragment root with `ListItem` children supporting
//! SelectionItem, plus a Selection pattern on the list and a notification when the
//! highlight moves.
//!
//! Providers hold only an `Arc<Mutex<Snapshot>>` (never window state), so UIA may call
//! them from any thread. `Select()` posts `WM_PIME_SELECT` to the window instead of
//! touching UI state directly.

#![allow(non_upper_case_globals)]

use std::ffi::c_void;
use std::sync::{Arc, Mutex, MutexGuard};

use windows::core::{implement, Error, IUnknown, IUnknownImpl, Interface, Result, BOOL, BSTR};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::SAFEARRAY;
use windows::Win32::System::Ole::{SafeArrayCreateVector, SafeArrayPutElement};
use windows::Win32::System::Variant::{VARIANT, VT_I4, VT_UNKNOWN};
use windows::Win32::UI::Accessibility::{
    IRawElementProviderFragment, IRawElementProviderFragmentRoot,
    IRawElementProviderFragmentRoot_Impl, IRawElementProviderFragment_Impl,
    IRawElementProviderSimple, IRawElementProviderSimple_Impl, ISelectionItemProvider,
    ISelectionItemProvider_Impl, ISelectionProvider, ISelectionProvider_Impl, NavigateDirection,
    NavigateDirection_FirstChild, NavigateDirection_LastChild, NavigateDirection_NextSibling,
    NavigateDirection_Parent, NavigateDirection_PreviousSibling, NotificationKind_ActionCompleted,
    NotificationProcessing_ImportantMostRecent, ProviderOptions,
    ProviderOptions_ServerSideProvider, UIA_AutomationIdPropertyId, UIA_ControlTypePropertyId,
    UIA_HasKeyboardFocusPropertyId, UIA_IsEnabledPropertyId, UIA_IsKeyboardFocusablePropertyId,
    UIA_ListControlTypeId, UIA_ListItemControlTypeId, UIA_NamePropertyId,
    UIA_SelectionItemPatternId, UIA_SelectionItem_ElementSelectedEventId, UIA_SelectionPatternId,
    UiaAppendRuntimeId, UiaClientsAreListening, UiaHostProviderFromHwnd, UiaRaiseAutomationEvent,
    UiaRaiseNotificationEvent, UiaRect, UIA_PATTERN_ID, UIA_PROPERTY_ID,
};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::geometry::Rect;

/// Posted by `ISelectionItemProvider::Select`; `wparam` = row index.
pub const WM_PIME_SELECT: u32 = WM_APP + 1;

#[derive(Default)]
pub struct Snapshot {
    pub hwnd: isize,
    pub items: Vec<String>,
    pub labels: Vec<String>,
    pub highlighted: usize,
    /// Row rectangles in client coordinates.
    pub rows: Vec<Rect>,
    /// Screen position of the client-area origin.
    pub origin: (i32, i32),
}

pub type Shared = Arc<Mutex<Snapshot>>;

fn lock(s: &Shared) -> MutexGuard<'_, Snapshot> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

const UIA_E_ELEMENTNOTAVAILABLE: windows::core::HRESULT =
    windows::core::HRESULT(0x80040201u32 as i32);
const UIA_E_INVALIDOPERATION: windows::core::HRESULT = windows::core::HRESULT(0x80131509u32 as i32);

/// "No such element": S_OK with a null out-pointer, which UIA reads as "no result".
fn none<T>() -> Result<T> {
    Err(Error::empty())
}

fn bounding(s: &Snapshot, r: Rect) -> UiaRect {
    UiaRect {
        left: f64::from(s.origin.0 + r.left),
        top: f64::from(s.origin.1 + r.top),
        width: f64::from(r.width()),
        height: f64::from(r.height()),
    }
}

fn int_array(vals: &[i32]) -> *mut SAFEARRAY {
    // SAFETY: vector of VT_I4 filled element by element.
    unsafe {
        let sa = SafeArrayCreateVector(VT_I4, 0, vals.len() as u32);
        for (i, v) in vals.iter().enumerate() {
            let _ = SafeArrayPutElement(sa, &(i as i32), v as *const i32 as *const c_void);
        }
        sa
    }
}

fn item_text(s: &Snapshot, i: usize) -> String {
    let label = s.labels.get(i).map_or("", String::as_str);
    if label.is_empty() {
        s.items[i].clone()
    } else {
        format!("{}, {}", s.items[i], label)
    }
}

// ---------------------------------------------------------------- list (root)

#[implement(
    IRawElementProviderSimple,
    IRawElementProviderFragment,
    IRawElementProviderFragmentRoot,
    ISelectionProvider
)]
struct ListProvider {
    shared: Shared,
}

fn item_fragment(shared: &Shared, index: usize) -> IRawElementProviderFragment {
    ItemProvider {
        shared: shared.clone(),
        index,
    }
    .into()
}

fn list_fragment(shared: &Shared) -> IRawElementProviderFragment {
    ListProvider {
        shared: shared.clone(),
    }
    .into()
}

impl IRawElementProviderSimple_Impl for ListProvider_Impl {
    fn ProviderOptions(&self) -> Result<ProviderOptions> {
        Ok(ProviderOptions_ServerSideProvider)
    }

    fn GetPatternProvider(&self, id: UIA_PATTERN_ID) -> Result<IUnknown> {
        if id == UIA_SelectionPatternId {
            let p: ISelectionProvider = self.to_object().to_interface();
            return p.cast();
        }
        none()
    }

    fn GetPropertyValue(&self, id: UIA_PROPERTY_ID) -> Result<VARIANT> {
        let s = lock(&self.shared);
        Ok(match id {
            UIA_ControlTypePropertyId => VARIANT::from(UIA_ListControlTypeId.0),
            UIA_NamePropertyId => VARIANT::from(BSTR::from("Candidates")),
            UIA_AutomationIdPropertyId => VARIANT::from(BSTR::from("PimeCandidateList")),
            UIA_IsEnabledPropertyId => VARIANT::from(true),
            UIA_IsKeyboardFocusablePropertyId | UIA_HasKeyboardFocusPropertyId => {
                VARIANT::from(false)
            }
            _ => {
                let _ = &s;
                VARIANT::default()
            }
        })
    }

    fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
        let hwnd = HWND(lock(&self.shared).hwnd as *mut c_void);
        // SAFETY: hwnd is our window.
        unsafe { UiaHostProviderFromHwnd(hwnd) }
    }
}

impl IRawElementProviderFragment_Impl for ListProvider_Impl {
    fn Navigate(&self, dir: NavigateDirection) -> Result<IRawElementProviderFragment> {
        let n = lock(&self.shared).items.len();
        match dir {
            NavigateDirection_FirstChild if n > 0 => Ok(item_fragment(&self.shared, 0)),
            NavigateDirection_LastChild if n > 0 => Ok(item_fragment(&self.shared, n - 1)),
            _ => none(), // parent is the HWND host
        }
    }

    fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
        // Fragment roots hosted in an HWND take their runtime id from the window.
        Ok(std::ptr::null_mut())
    }

    fn BoundingRectangle(&self) -> Result<UiaRect> {
        let s = lock(&self.shared);
        let (w, h) = s
            .rows
            .iter()
            .fold((0, 0), |(w, h), r| (w.max(r.right), h.max(r.bottom)));
        let all = Rect::new(0, 0, w + 4, h + 4);
        Ok(bounding(&s, all))
    }

    fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
        Ok(std::ptr::null_mut())
    }

    fn SetFocus(&self) -> Result<()> {
        Ok(())
    }

    fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
        Ok(ListProvider {
            shared: self.shared.clone(),
        }
        .into())
    }
}

impl IRawElementProviderFragmentRoot_Impl for ListProvider_Impl {
    fn ElementProviderFromPoint(&self, x: f64, y: f64) -> Result<IRawElementProviderFragment> {
        let s = lock(&self.shared);
        let (cx, cy) = (x as i32 - s.origin.0, y as i32 - s.origin.1);
        match s.rows.iter().position(|r| r.contains(cx, cy)) {
            Some(i) => Ok(item_fragment(&self.shared, i)),
            None => Ok(list_fragment(&self.shared)),
        }
    }

    fn GetFocus(&self) -> Result<IRawElementProviderFragment> {
        let s = lock(&self.shared);
        if s.items.is_empty() {
            return none();
        }
        Ok(item_fragment(
            &self.shared,
            s.highlighted.min(s.items.len() - 1),
        ))
    }
}

impl ISelectionProvider_Impl for ListProvider_Impl {
    fn GetSelection(&self) -> Result<*mut SAFEARRAY> {
        let s = lock(&self.shared);
        let count = u32::from(!s.items.is_empty());
        // SAFETY: vector of IUnknown; PutElement AddRefs the element.
        unsafe {
            let sa = SafeArrayCreateVector(VT_UNKNOWN, 0, count);
            if count == 1 {
                let item: IRawElementProviderSimple = ItemProvider {
                    shared: self.shared.clone(),
                    index: s.highlighted,
                }
                .into();
                let unk: IUnknown = item.cast()?;
                SafeArrayPutElement(sa, &0, unk.as_raw())?;
            }
            Ok(sa)
        }
    }

    fn CanSelectMultiple(&self) -> Result<BOOL> {
        Ok(BOOL(0))
    }

    fn IsSelectionRequired(&self) -> Result<BOOL> {
        Ok(BOOL(1))
    }
}

// ---------------------------------------------------------------- items

#[implement(
    IRawElementProviderSimple,
    IRawElementProviderFragment,
    ISelectionItemProvider
)]
struct ItemProvider {
    shared: Shared,
    index: usize,
}

impl IRawElementProviderSimple_Impl for ItemProvider_Impl {
    fn ProviderOptions(&self) -> Result<ProviderOptions> {
        Ok(ProviderOptions_ServerSideProvider)
    }

    fn GetPatternProvider(&self, id: UIA_PATTERN_ID) -> Result<IUnknown> {
        if id == UIA_SelectionItemPatternId {
            let p: ISelectionItemProvider = self.to_object().to_interface();
            return p.cast();
        }
        none()
    }

    fn GetPropertyValue(&self, id: UIA_PROPERTY_ID) -> Result<VARIANT> {
        let s = lock(&self.shared);
        if self.index >= s.items.len() {
            return Err(UIA_E_ELEMENTNOTAVAILABLE.into());
        }
        Ok(match id {
            UIA_ControlTypePropertyId => VARIANT::from(UIA_ListItemControlTypeId.0),
            UIA_NamePropertyId => VARIANT::from(BSTR::from(item_text(&s, self.index))),
            UIA_AutomationIdPropertyId => {
                VARIANT::from(BSTR::from(format!("PimeCandidate{}", self.index + 1)))
            }
            UIA_IsEnabledPropertyId => VARIANT::from(true),
            UIA_IsKeyboardFocusablePropertyId | UIA_HasKeyboardFocusPropertyId => {
                VARIANT::from(false)
            }
            _ => VARIANT::default(),
        })
    }

    fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
        none()
    }
}

impl IRawElementProviderFragment_Impl for ItemProvider_Impl {
    fn Navigate(&self, dir: NavigateDirection) -> Result<IRawElementProviderFragment> {
        let n = lock(&self.shared).items.len();
        match dir {
            NavigateDirection_Parent => Ok(list_fragment(&self.shared)),
            NavigateDirection_NextSibling if self.index + 1 < n => {
                Ok(item_fragment(&self.shared, self.index + 1))
            }
            NavigateDirection_PreviousSibling if self.index > 0 => {
                Ok(item_fragment(&self.shared, self.index - 1))
            }
            _ => none(),
        }
    }

    fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
        Ok(int_array(&[
            UiaAppendRuntimeId as i32,
            self.index as i32 + 1,
        ]))
    }

    fn BoundingRectangle(&self) -> Result<UiaRect> {
        let s = lock(&self.shared);
        match s.rows.get(self.index) {
            Some(r) => Ok(bounding(&s, *r)),
            None => Err(UIA_E_ELEMENTNOTAVAILABLE.into()),
        }
    }

    fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
        Ok(std::ptr::null_mut())
    }

    fn SetFocus(&self) -> Result<()> {
        Ok(())
    }

    fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
        Ok(ListProvider {
            shared: self.shared.clone(),
        }
        .into())
    }
}

impl ISelectionItemProvider_Impl for ItemProvider_Impl {
    fn Select(&self) -> Result<()> {
        let s = lock(&self.shared);
        if self.index >= s.items.len() {
            return Err(UIA_E_ELEMENTNOTAVAILABLE.into());
        }
        // SAFETY: PostMessage is thread-safe; a dead hwnd just fails.
        unsafe {
            let _ = PostMessageW(
                Some(HWND(s.hwnd as *mut c_void)),
                WM_PIME_SELECT,
                WPARAM(self.index),
                LPARAM(0),
            );
        }
        Ok(())
    }

    fn AddToSelection(&self) -> Result<()> {
        Err(UIA_E_INVALIDOPERATION.into())
    }

    fn RemoveFromSelection(&self) -> Result<()> {
        Err(UIA_E_INVALIDOPERATION.into())
    }

    fn IsSelected(&self) -> Result<BOOL> {
        Ok(BOOL::from(lock(&self.shared).highlighted == self.index))
    }

    fn SelectionContainer(&self) -> Result<IRawElementProviderSimple> {
        Ok(ListProvider {
            shared: self.shared.clone(),
        }
        .into())
    }
}

// ---------------------------------------------------------------- entry points

pub fn root_provider(shared: &Shared) -> IRawElementProviderSimple {
    ListProvider {
        shared: shared.clone(),
    }
    .into()
}

/// Announce the newly highlighted candidate ("<text>, 2 of 5"). No-op without listeners.
pub fn raise_highlight(shared: &Shared, index: usize) {
    // SAFETY: plain UIA calls with providers we own.
    unsafe {
        if !UiaClientsAreListening().as_bool() {
            return;
        }
        let (text, count) = {
            let s = lock(shared);
            if index >= s.items.len() {
                return;
            }
            (item_text(&s, index), s.items.len())
        };
        let item: IRawElementProviderSimple = ItemProvider {
            shared: shared.clone(),
            index,
        }
        .into();
        let _ = UiaRaiseAutomationEvent(&item, UIA_SelectionItem_ElementSelectedEventId);
        let list = root_provider(shared);
        let msg = BSTR::from(format!("{}, {} of {}", text, index + 1, count));
        let _ = UiaRaiseNotificationEvent(
            &list,
            NotificationKind_ActionCompleted,
            NotificationProcessing_ImportantMostRecent,
            &msg,
            &BSTR::from("PimeCandidateHighlight"),
        );
    }
}
