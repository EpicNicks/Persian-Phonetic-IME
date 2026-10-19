//! Edit sessions: the only place the document is touched. Executes an [`EditPlan`] against the
//! composition stored in the service *at run time*, because with TF_ES_ASYNCDONTCARE a session
//! may run after later keys were already processed.

use std::mem::ManuallyDrop;

use windows::Win32::Foundation::E_UNEXPECTED;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::TextServices::{
    ITfComposition, ITfCompositionSink, ITfContext, ITfContextComposition, ITfEditSession,
    ITfEditSession_Impl, ITfInsertAtSelection, ITfRange, GUID_PROP_ATTRIBUTE,
    INSERT_TEXT_AT_SELECTION_FLAGS, TF_AE_NONE, TF_ANCHOR_END, TF_ANCHOR_START,
    TF_CONTEXT_EDIT_CONTEXT_FLAGS, TF_DEFAULT_SELECTION, TF_ES_ASYNC, TF_ES_ASYNCDONTCARE,
    TF_ES_READ, TF_ES_READWRITE, TF_ES_SYNC, TF_IAS_QUERYONLY, TF_SELECTION, TF_SELECTIONSTYLE,
    TF_ST_CORRECTION,
};
use windows_core::{implement, ComObject, Interface, HRESULT};

use crate::anchor;
use crate::composition::{utf16_len, CandStep, DocStep, EditPlan};
use crate::exports::{guard, ObjToken};
use crate::service::TextService;

const ZWNJ_UNIT: u16 = 0x200C;

pub enum Job {
    Apply(EditPlan),
    /// The layout changed (or was missing when the popup opened): re-anchor the open popup.
    Reanchor,
}

/// How to request the session.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Must run before we return (commit before a passed-through key).
    Sync,
    /// Run now if the document can be locked, else queue (key handlers).
    AsyncDontCare,
    /// Always queued: used from callbacks where re-entering would be unsafe (popup clicks).
    Async,
}

#[implement(ITfEditSession)]
struct EditSession {
    service: ComObject<TextService>,
    context: ITfContext,
    job: Job,
    generation: u32,
    _tok: ObjToken,
}

impl ITfEditSession_Impl for EditSession_Impl {
    fn DoEditSession(&self, ec: u32) -> windows_core::Result<()> {
        guard(|| unsafe {
            let svc = &self.service;
            // A plan queued before the host terminated our composition must not resurrect it
            // (or re-insert text the host already kept).
            if self.generation != svc.generation() {
                svc.hide_candidates();
                return Ok(());
            }
            match &self.job {
                Job::Apply(plan) => apply(svc, &self.context, ec, plan),
                Job::Reanchor => {
                    reanchor(svc, &self.context, ec);
                    Ok(())
                }
            }
        })
    }
}

/// Returns the session's own HRESULT when it ran synchronously, TF_S_ASYNC when queued.
pub fn request(
    service: ComObject<TextService>,
    context: &ITfContext,
    job: Job,
    mode: Mode,
) -> windows_core::Result<HRESULT> {
    let client_id = service.client_id();
    let generation = service.generation();
    let read_only = matches!(job, Job::Reanchor);
    let session: ITfEditSession = EditSession {
        service,
        context: context.clone(),
        job,
        generation,
        _tok: ObjToken::new(),
    }
    .into();
    let access = if read_only {
        TF_ES_READ
    } else {
        TF_ES_READWRITE
    };
    let flags: TF_CONTEXT_EDIT_CONTEXT_FLAGS = access
        | match mode {
            Mode::Sync => TF_ES_SYNC,
            Mode::AsyncDontCare => TF_ES_ASYNCDONTCARE,
            Mode::Async => TF_ES_ASYNC,
        };
    unsafe { context.RequestEditSession(client_id, &session, flags) }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

unsafe fn apply(
    svc: &ComObject<TextService>,
    ctx: &ITfContext,
    ec: u32,
    plan: &EditPlan,
) -> windows_core::Result<()> {
    let doc = (|| {
        for step in &plan.doc {
            match step {
                DocStep::Finish(text) => finish_or_insert(svc, ctx, ec, text)?,
                DocStep::ToggleZwnj => toggle_zwnj(svc, ctx, ec)?,
                DocStep::Compose { text, pending } => compose(svc, ctx, ec, text, pending.clone())?,
            }
        }
        windows_core::Result::Ok(())
    })();
    if doc.is_err() {
        svc.hide_candidates();
        return doc;
    }
    match &plan.candidates {
        CandStep::Keep => {}
        CandStep::Hide => svc.hide_candidates(),
        CandStep::Show(list) => {
            let range = svc.composition().and_then(|c| c.GetRange().ok());
            let units = match &range {
                Some(r) => range_units(ec, r),
                None => 0,
            };
            let a = anchor::compute(ctx, ec, range.as_ref());
            svc.show_candidates(list, a, plan.side, units);
        }
    }
    Ok(())
}

unsafe fn reanchor(svc: &ComObject<TextService>, ctx: &ITfContext, ec: u32) {
    let Some(range) = svc.composition().and_then(|c| c.GetRange().ok()) else {
        return;
    };
    let a = anchor::compute(ctx, ec, Some(&range));
    svc.reanchor_candidates(a);
}

/// Length of a range in UTF-16 units (bounded: a composition is one word).
unsafe fn range_units(ec: u32, r: &ITfRange) -> usize {
    let mut buf = [0u16; 256];
    let mut got = 0u32;
    if r.GetText(ec, 0, &mut buf, &mut got).is_ok() {
        got as usize
    } else {
        0
    }
}

unsafe fn compose(
    svc: &ComObject<TextService>,
    ctx: &ITfContext,
    ec: u32,
    text: &str,
    pending: Option<std::ops::Range<usize>>,
) -> windows_core::Result<()> {
    let comp = match svc.composition() {
        Some(c) => c,
        None => {
            let c = start_composition(svc, ctx, ec)?;
            svc.set_composition(Some(c.clone()));
            c
        }
    };
    let range = comp.GetRange()?;
    range.SetText(ec, TF_ST_CORRECTION, &wide(text))?;
    // Attributes are cosmetic: a host that rejects them must not stop composing.
    let _ = set_attributes(svc, ctx, ec, &range, pending, utf16_len(text));
    caret_to_end(ctx, ec, &range)
}

/// End the open composition with `text`, or insert `text` if none is open.
unsafe fn finish_or_insert(
    svc: &ComObject<TextService>,
    ctx: &ITfContext,
    ec: u32,
    text: &str,
) -> windows_core::Result<()> {
    if let Some(comp) = svc.composition() {
        svc.set_composition(None);
        return finish(ctx, ec, &comp, text);
    }
    if text.is_empty() {
        return Ok(());
    }
    insert(svc, ctx, ec, text)
}

unsafe fn toggle_zwnj(
    svc: &ComObject<TextService>,
    ctx: &ITfContext,
    ec: u32,
) -> windows_core::Result<()> {
    // The one deliberate read-back (PLAN 2.2), in the same session as the write so nothing can
    // change in between. Any failure falls back to inserting.
    if let Some((range, unit)) = char_before_caret(ctx, ec) {
        if unit == ZWNJ_UNIT && range.SetText(ec, 0, &[]).is_ok() {
            return Ok(());
        }
    }
    insert(svc, ctx, ec, "\u{200C}")
}

unsafe fn start_composition(
    svc: &ComObject<TextService>,
    ctx: &ITfContext,
    ec: u32,
) -> windows_core::Result<ITfComposition> {
    let range =
        ctx.cast::<ITfInsertAtSelection>()?
            .InsertTextAtSelection(ec, TF_IAS_QUERYONLY, &[])?;
    let sink: ITfCompositionSink = svc.to_interface();
    // A host may refuse (S_OK + NULL composition). windows-rs reports that as
    // Err(Error::empty()), whose HRESULT is S_OK: map it to a real failure.
    ctx.cast::<ITfContextComposition>()?
        .StartComposition(ec, &range, &sink)
        .map_err(|e| {
            if e.code().is_ok() {
                E_UNEXPECTED.into()
            } else {
                e
            }
        })
}

unsafe fn set_attributes(
    svc: &ComObject<TextService>,
    ctx: &ITfContext,
    ec: u32,
    range: &ITfRange,
    pending: Option<std::ops::Range<usize>>,
    len: usize,
) -> windows_core::Result<()> {
    let (input, pending_atom) = svc.atoms();
    let prop = ctx.GetProperty(&GUID_PROP_ATTRIBUTE)?;
    if input != 0 {
        prop.SetValue(ec, range, &VARIANT::from(input as i32))?;
    }
    if let Some(p) = pending.filter(|p| pending_atom != 0 && p.start < p.end && p.end <= len) {
        let sub = range.Clone()?;
        sub.Collapse(ec, TF_ANCHOR_START)?;
        let mut moved = 0i32;
        sub.ShiftEnd(ec, p.end as i32, &mut moved, std::ptr::null())?;
        sub.ShiftStart(ec, p.start as i32, &mut moved, std::ptr::null())?;
        prop.SetValue(ec, &sub, &VARIANT::from(pending_atom as i32))?;
    }
    Ok(())
}

/// Final text, attribute cleared, caret after it, composition ended.
unsafe fn finish(
    ctx: &ITfContext,
    ec: u32,
    comp: &ITfComposition,
    text: &str,
) -> windows_core::Result<()> {
    let range = comp.GetRange()?;
    range.SetText(ec, TF_ST_CORRECTION, &wide(text))?;
    if let Ok(prop) = ctx.GetProperty(&GUID_PROP_ATTRIBUTE) {
        let _ = prop.Clear(ec, &range);
    }
    caret_to_end(ctx, ec, &range)?;
    comp.EndComposition(ec)
}

/// Plain insertion, done as start -> set -> end composition (best CUAS compatibility, PLAN 5.2),
/// falling back to InsertTextAtSelection if the host refuses a composition.
unsafe fn insert(
    svc: &ComObject<TextService>,
    ctx: &ITfContext,
    ec: u32,
    text: &str,
) -> windows_core::Result<()> {
    match start_composition(svc, ctx, ec) {
        Ok(comp) => finish(ctx, ec, &comp, text),
        Err(_) => {
            let range = ctx.cast::<ITfInsertAtSelection>()?.InsertTextAtSelection(
                ec,
                INSERT_TEXT_AT_SELECTION_FLAGS(0),
                &wide(text),
            )?;
            caret_to_end(ctx, ec, &range)
        }
    }
}

unsafe fn caret_to_end(ctx: &ITfContext, ec: u32, range: &ITfRange) -> windows_core::Result<()> {
    let caret = range.Clone()?;
    caret.Collapse(ec, TF_ANCHOR_END)?;
    let mut sel = [TF_SELECTION {
        range: ManuallyDrop::new(Some(caret)),
        style: TF_SELECTIONSTYLE {
            ase: TF_AE_NONE,
            fInterimChar: false.into(),
        },
    }];
    let r = ctx.SetSelection(ec, &sel);
    // TF_SELECTION holds the range in ManuallyDrop (ABI struct): release it by hand or every
    // key leaks a range.
    ManuallyDrop::drop(&mut sel[0].range);
    r
}

/// The current default selection's range (owned), if exactly one was returned.
pub(crate) unsafe fn selection(ctx: &ITfContext, ec: u32) -> Option<ITfRange> {
    let mut sel = [TF_SELECTION::default()];
    let mut fetched = 0u32;
    let ok = ctx
        .GetSelection(ec, TF_DEFAULT_SELECTION, &mut sel, &mut fetched)
        .is_ok();
    let range = ManuallyDrop::take(&mut sel[0].range);
    if !ok || fetched != 1 {
        return None;
    }
    range
}

/// Returns (range covering the previous char, that UTF-16 unit) when the selection is empty.
unsafe fn char_before_caret(ctx: &ITfContext, ec: u32) -> Option<(ITfRange, u16)> {
    let range = selection(ctx, ec)?;
    if !range.IsEmpty(ec).ok()?.as_bool() {
        return None;
    }
    let mut moved = 0i32;
    range
        .ShiftStart(ec, -1, &mut moved, std::ptr::null())
        .ok()?;
    if moved != -1 {
        return None;
    }
    let mut buf = [0u16; 2];
    let mut got = 0u32;
    range.GetText(ec, 0, &mut buf, &mut got).ok()?;
    (got == 1).then_some((range, buf[0]))
}

/// True when the selection lies inside `comp` (inclusive of its ends) or cannot be read.
pub(crate) unsafe fn selection_inside(ctx: &ITfContext, ec: u32, comp: &ITfRange) -> bool {
    let Some(sel) = selection(ctx, ec) else {
        return true;
    };
    let start_ok = sel
        .CompareStart(ec, comp, TF_ANCHOR_START)
        .map_or(true, |c| c >= 0);
    let end_ok = sel
        .CompareEnd(ec, comp, TF_ANCHOR_END)
        .map_or(true, |c| c <= 0);
    start_ok && end_ok
}
