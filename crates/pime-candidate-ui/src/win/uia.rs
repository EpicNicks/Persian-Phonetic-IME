//! UI Automation provider: a `List` fragment root with `ListItem` children supporting
//! SelectionItem, plus a Selection pattern on the list and a notification when the
//! highlight moves.
//!
//! Providers hold only an `Arc<Mutex<Snapshot>>` (never window state), so UIA may call
//! them from any thread. `Select()` posts `WM_PIME_SELECT` to the window instead of
//! touching UI state directly.

#![allow(non_upper_case_globals)]

use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex, MutexGuard};

use windows::core::{implement, Error, IUnknown, IUnknownImpl, Interface, Result, BOOL, BSTR};
use windows::Win32::Foundation::{E_FAIL, HWND, LPARAM, WPARAM};
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
    UIA_HasKeyboardFocusPropertyId, UIA_HelpTextPropertyId, UIA_IsEnabledPropertyId,
    UIA_IsKeyboardFocusablePropertyId, UIA_ListControlTypeId, UIA_ListItemControlTypeId,
    UIA_NamePropertyId, UIA_SelectionItemPatternId, UIA_SelectionItem_ElementSelectedEventId,
    UIA_SelectionPatternId, UiaAppendRuntimeId, UiaClientsAreListening, UiaHostProviderFromHwnd,
    UiaRaiseAutomationEvent, UiaRaiseNotificationEvent, UiaRect, UIA_PATTERN_ID, UIA_PROPERTY_ID,
};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::geometry::Rect;
use crate::objects::ComToken;

/// Posted by `ISelectionItemProvider::Select`; `wparam` = row index.
pub const WM_PIME_SELECT: u32 = WM_APP + 1;

#[derive(Default)]
pub struct Snapshot {
    pub hwnd: isize,
    pub items: Vec<String>,
    pub labels: Vec<String>,
    /// Rows from this index on are word completions (below the divider).
    pub first_completion: usize,
    /// Selected row; `None` when nothing is highlighted.
    pub highlighted: Option<usize>,
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

/// Every provider method is a COM entry point on a UIA thread: a panic must not unwind into
/// the client. The snapshot mutex recovers from poisoning, so later calls keep working.
fn guard<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| Err(E_FAIL.into()))
}

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

fn is_completion(s: &Snapshot, i: usize) -> bool {
    i >= s.first_completion && i < s.items.len()
}

/// The selected row, if it is a live row.
fn selected(s: &Snapshot) -> Option<usize> {
    s.highlighted.filter(|&i| i < s.items.len())
}

/// Highlight announcement: "<text>, 2 of 5", with ", completion" after the text for
/// completion rows.
fn highlight_message(s: &Snapshot, i: usize) -> String {
    let kind = if is_completion(s, i) {
        ", completion"
    } else {
        ""
    };
    format!("{}{kind}, {} of {}", item_text(s, i), i + 1, s.items.len())
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
    _tok: ComToken,
}

impl ListProvider {
    fn new(shared: &Shared) -> Self {
        Self {
            shared: shared.clone(),
            _tok: ComToken::new(),
        }
    }
}

fn item_fragment(shared: &Shared, index: usize) -> IRawElementProviderFragment {
    ItemProvider::new(shared, index).into()
}

fn list_fragment(shared: &Shared) -> IRawElementProviderFragment {
    ListProvider::new(shared).into()
}

impl IRawElementProviderSimple_Impl for ListProvider_Impl {
    fn ProviderOptions(&self) -> Result<ProviderOptions> {
        guard(|| Ok(ProviderOptions_ServerSideProvider))
    }

    fn GetPatternProvider(&self, id: UIA_PATTERN_ID) -> Result<IUnknown> {
        guard(|| {
            if id == UIA_SelectionPatternId {
                let p: ISelectionProvider = self.to_object().to_interface();
                return p.cast();
            }
            none()
        })
    }

    fn GetPropertyValue(&self, id: UIA_PROPERTY_ID) -> Result<VARIANT> {
        guard(|| {
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
        })
    }

    fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
        guard(|| {
            let hwnd = HWND(lock(&self.shared).hwnd as *mut c_void);
            // SAFETY: hwnd is our window.
            unsafe { UiaHostProviderFromHwnd(hwnd) }
        })
    }
}

impl IRawElementProviderFragment_Impl for ListProvider_Impl {
    fn Navigate(&self, dir: NavigateDirection) -> Result<IRawElementProviderFragment> {
        guard(|| {
            let n = lock(&self.shared).items.len();
            match dir {
                NavigateDirection_FirstChild if n > 0 => Ok(item_fragment(&self.shared, 0)),
                NavigateDirection_LastChild if n > 0 => Ok(item_fragment(&self.shared, n - 1)),
                _ => none(), // parent is the HWND host
            }
        })
    }

    fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
        guard(|| {
            // Fragment roots hosted in an HWND take their runtime id from the window.
            Ok(std::ptr::null_mut())
        })
    }

    fn BoundingRectangle(&self) -> Result<UiaRect> {
        guard(|| {
            let s = lock(&self.shared);
            let (w, h) = s
                .rows
                .iter()
                .fold((0, 0), |(w, h), r| (w.max(r.right), h.max(r.bottom)));
            let all = Rect::new(0, 0, w + 4, h + 4);
            Ok(bounding(&s, all))
        })
    }

    fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
        guard(|| Ok(std::ptr::null_mut()))
    }

    fn SetFocus(&self) -> Result<()> {
        guard(|| Ok(()))
    }

    fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
        guard(|| Ok(ListProvider::new(&self.shared).into()))
    }
}

impl IRawElementProviderFragmentRoot_Impl for ListProvider_Impl {
    fn ElementProviderFromPoint(&self, x: f64, y: f64) -> Result<IRawElementProviderFragment> {
        guard(|| {
            let s = lock(&self.shared);
            let (cx, cy) = (x as i32 - s.origin.0, y as i32 - s.origin.1);
            match s.rows.iter().position(|r| r.contains(cx, cy)) {
                Some(i) => Ok(item_fragment(&self.shared, i)),
                None => Ok(list_fragment(&self.shared)),
            }
        })
    }

    fn GetFocus(&self) -> Result<IRawElementProviderFragment> {
        guard(|| match selected(&lock(&self.shared)) {
            Some(i) => Ok(item_fragment(&self.shared, i)),
            None => none(),
        })
    }
}

impl ISelectionProvider_Impl for ListProvider_Impl {
    fn GetSelection(&self) -> Result<*mut SAFEARRAY> {
        guard(|| {
            let sel = selected(&lock(&self.shared));
            let count = u32::from(sel.is_some());
            // SAFETY: vector of IUnknown; PutElement AddRefs the element.
            unsafe {
                let sa = SafeArrayCreateVector(VT_UNKNOWN, 0, count);
                if let Some(i) = sel {
                    let item: IRawElementProviderSimple = ItemProvider::new(&self.shared, i).into();
                    let unk: IUnknown = item.cast()?;
                    SafeArrayPutElement(sa, &0, unk.as_raw())?;
                }
                Ok(sa)
            }
        })
    }

    fn CanSelectMultiple(&self) -> Result<BOOL> {
        guard(|| Ok(BOOL(0)))
    }

    fn IsSelectionRequired(&self) -> Result<BOOL> {
        // A completions-only popup starts with nothing selected.
        guard(|| Ok(BOOL(0)))
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
    _tok: ComToken,
}

impl ItemProvider {
    fn new(shared: &Shared, index: usize) -> Self {
        Self {
            shared: shared.clone(),
            index,
            _tok: ComToken::new(),
        }
    }
}

impl IRawElementProviderSimple_Impl for ItemProvider_Impl {
    fn ProviderOptions(&self) -> Result<ProviderOptions> {
        guard(|| Ok(ProviderOptions_ServerSideProvider))
    }

    fn GetPatternProvider(&self, id: UIA_PATTERN_ID) -> Result<IUnknown> {
        guard(|| {
            if id == UIA_SelectionItemPatternId {
                let p: ISelectionItemProvider = self.to_object().to_interface();
                return p.cast();
            }
            none()
        })
    }

    fn GetPropertyValue(&self, id: UIA_PROPERTY_ID) -> Result<VARIANT> {
        guard(|| {
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
                UIA_HelpTextPropertyId if is_completion(&s, self.index) => {
                    VARIANT::from(BSTR::from("completion"))
                }
                UIA_IsEnabledPropertyId => VARIANT::from(true),
                UIA_IsKeyboardFocusablePropertyId | UIA_HasKeyboardFocusPropertyId => {
                    VARIANT::from(false)
                }
                _ => VARIANT::default(),
            })
        })
    }

    fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
        guard(none)
    }
}

impl IRawElementProviderFragment_Impl for ItemProvider_Impl {
    fn Navigate(&self, dir: NavigateDirection) -> Result<IRawElementProviderFragment> {
        guard(|| {
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
        })
    }

    fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
        guard(|| {
            Ok(int_array(&[
                UiaAppendRuntimeId as i32,
                self.index as i32 + 1,
            ]))
        })
    }

    fn BoundingRectangle(&self) -> Result<UiaRect> {
        guard(|| {
            let s = lock(&self.shared);
            match s.rows.get(self.index) {
                Some(r) => Ok(bounding(&s, *r)),
                None => Err(UIA_E_ELEMENTNOTAVAILABLE.into()),
            }
        })
    }

    fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
        guard(|| Ok(std::ptr::null_mut()))
    }

    fn SetFocus(&self) -> Result<()> {
        guard(|| Ok(()))
    }

    fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
        guard(|| Ok(ListProvider::new(&self.shared).into()))
    }
}

impl ISelectionItemProvider_Impl for ItemProvider_Impl {
    fn Select(&self) -> Result<()> {
        guard(|| {
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
        })
    }

    fn AddToSelection(&self) -> Result<()> {
        guard(|| Err(UIA_E_INVALIDOPERATION.into()))
    }

    fn RemoveFromSelection(&self) -> Result<()> {
        guard(|| Err(UIA_E_INVALIDOPERATION.into()))
    }

    fn IsSelected(&self) -> Result<BOOL> {
        guard(|| {
            Ok(BOOL::from(
                lock(&self.shared).highlighted == Some(self.index),
            ))
        })
    }

    fn SelectionContainer(&self) -> Result<IRawElementProviderSimple> {
        guard(|| Ok(ListProvider::new(&self.shared).into()))
    }
}

// ---------------------------------------------------------------- entry points

pub fn root_provider(shared: &Shared) -> IRawElementProviderSimple {
    ListProvider::new(shared).into()
}

/// Announce the newly highlighted candidate ("<text>, 2 of 5"). No-op without listeners.
pub fn raise_highlight(shared: &Shared, index: usize) {
    // SAFETY: plain UIA calls with providers we own.
    unsafe {
        if !UiaClientsAreListening().as_bool() {
            return;
        }
        let msg = {
            let s = lock(shared);
            if index >= s.items.len() {
                return;
            }
            highlight_message(&s, index)
        };
        let item: IRawElementProviderSimple = ItemProvider::new(shared, index).into();
        let _ = UiaRaiseAutomationEvent(&item, UIA_SelectionItem_ElementSelectedEventId);
        let list = root_provider(shared);
        let msg = BSTR::from(msg);
        let _ = UiaRaiseNotificationEvent(
            &list,
            NotificationKind_ActionCompleted,
            NotificationProcessing_ImportantMostRecent,
            &msg,
            &BSTR::from("PimeCandidateHighlight"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The live count is process-global; keep provider-creating tests apart.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn shared() -> Shared {
        Arc::new(Mutex::new(Snapshot {
            items: vec!["a".into(), "b".into()],
            labels: vec![String::new(), "bee".into()],
            highlighted: Some(1),
            rows: vec![Rect::new(0, 0, 10, 10), Rect::new(0, 10, 10, 20)],
            ..Default::default()
        }))
    }

    #[test]
    fn providers_are_counted_while_alive() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let sh = shared();
        let before = crate::live_com_objects();
        let root = root_provider(&sh);
        let frag: IRawElementProviderFragment = root.cast().unwrap();
        // SAFETY: calls on our own providers.
        let item = unsafe { frag.Navigate(NavigateDirection_LastChild) }.unwrap();
        assert_eq!(crate::live_com_objects(), before + 2);
        let simple: IRawElementProviderSimple = item.cast().unwrap();
        // SAFETY: as above.
        let name = unsafe { simple.GetPropertyValue(UIA_NamePropertyId) }.unwrap();
        // SAFETY: a VT_BSTR variant built by GetPropertyValue.
        let text = unsafe { name.Anonymous.Anonymous.Anonymous.bstrVal.to_string() };
        assert_eq!(text, "b, bee");
        drop((root, frag, item, simple));
        assert_eq!(crate::live_com_objects(), before);
    }

    fn string_prop(p: &IRawElementProviderSimple, id: UIA_PROPERTY_ID) -> String {
        // SAFETY: call on our own provider.
        let v = unsafe { p.GetPropertyValue(id) }.unwrap();
        if v.vt() != windows::Win32::System::Variant::VT_BSTR {
            return String::new();
        }
        // SAFETY: a VT_BSTR variant built by GetPropertyValue.
        unsafe { v.Anonymous.Anonymous.Anonymous.bstrVal.to_string() }
    }

    fn two_sections() -> Shared {
        Arc::new(Mutex::new(Snapshot {
            items: vec!["س".into(), "ص".into(), "سلام".into(), "سلامت".into()],
            labels: vec!["sin".into(), "sad".into()],
            first_completion: 2,
            highlighted: Some(3),
            rows: (0..4)
                .map(|i| Rect::new(0, i * 10, 10, i * 10 + 10))
                .collect(),
            ..Default::default()
        }))
    }

    #[test]
    fn completion_rows_are_list_items_with_help_text() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let sh = two_sections();
        let props = |i| {
            let p: IRawElementProviderSimple = ItemProvider::new(&sh, i).into();
            (
                string_prop(&p, UIA_NamePropertyId),
                string_prop(&p, UIA_HelpTextPropertyId),
            )
        };
        assert_eq!(props(1), ("ص, sad".to_owned(), String::new()));
        assert_eq!(props(2), ("سلام".to_owned(), "completion".to_owned()));
        assert_eq!(props(3).1, "completion");
        // Siblings run across the divider.
        let item: IRawElementProviderFragment = ItemProvider::new(&sh, 1).into();
        // SAFETY: call on our own provider.
        let next = unsafe { item.Navigate(NavigateDirection_NextSibling) }.unwrap();
        let next: IRawElementProviderSimple = next.cast().unwrap();
        assert_eq!(string_prop(&next, UIA_NamePropertyId), "سلام");
        let s = lock(&sh);
        assert_eq!(highlight_message(&s, 3), "سلامت, completion, 4 of 4");
        assert_eq!(highlight_message(&s, 1), "ص, sad, 2 of 4");
    }

    #[test]
    fn selection_follows_combined_rows() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let sh = two_sections();
        let sel: ISelectionItemProvider = ItemProvider::new(&sh, 3).into();
        let other: ISelectionItemProvider = ItemProvider::new(&sh, 0).into();
        let list: ISelectionProvider = ListProvider::new(&sh).into();
        let count = |sa: *mut SAFEARRAY| {
            // SAFETY: a vector SAFEARRAY returned by GetSelection; destroyed after reading.
            unsafe {
                let n = (*sa).rgsabound[0].cElements;
                let _ = windows::Win32::System::Ole::SafeArrayDestroy(sa);
                n
            }
        };
        // SAFETY: calls on our own providers.
        unsafe {
            assert!(sel.IsSelected().unwrap().as_bool());
            assert!(!other.IsSelected().unwrap().as_bool());
            assert!(!list.IsSelectionRequired().unwrap().as_bool());
            assert_eq!(count(list.GetSelection().unwrap()), 1);
            lock(&sh).highlighted = None;
            assert!(!sel.IsSelected().unwrap().as_bool());
            assert_eq!(count(list.GetSelection().unwrap()), 0, "nothing selected");
        }
    }

    #[test]
    fn guard_turns_panic_into_e_fail_and_poisoned_lock_recovers() {
        let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let r: Result<()> = guard(|| panic!("boom"));
        assert_eq!(r.unwrap_err().code(), E_FAIL);
        let sh = shared();
        let sh2 = sh.clone();
        let _ = std::thread::spawn(move || {
            let _g = sh2.lock().unwrap();
            panic!("poison the snapshot");
        })
        .join();
        assert!(sh.is_poisoned());
        let item: ISelectionItemProvider = ItemProvider::new(&sh, 1).into();
        // SAFETY: call on our own provider.
        assert!(unsafe { item.IsSelected() }.unwrap().as_bool());
    }
}
