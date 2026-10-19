//! Edit sessions: the only place the document is touched. Each key produces one `Op`; the
//! session applies it against the composition stored in the service *at run time*, because with
//! TF_ES_ASYNCDONTCARE the session may run after later keys were already processed by the engine.

use std::mem::ManuallyDrop;

use windows::Win32::Foundation::E_UNEXPECTED;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::TextServices::{
    ITfComposition, ITfCompositionSink, ITfContext, ITfContextComposition, ITfEditSession,
    ITfEditSession_Impl, ITfInsertAtSelection, ITfRange, GUID_PROP_ATTRIBUTE,
    INSERT_TEXT_AT_SELECTION_FLAGS, TF_AE_NONE, TF_ANCHOR_END, TF_DEFAULT_SELECTION,
    TF_ES_ASYNCDONTCARE, TF_ES_READWRITE, TF_ES_SYNC, TF_IAS_QUERYONLY, TF_SELECTION,
    TF_SELECTIONSTYLE, TF_ST_CORRECTION,
};
use windows_core::{implement, ComObject, Interface, HRESULT};

use crate::engine::ZWNJ;
use crate::exports::{guard, ObjToken};
use crate::service::TextService;

#[derive(Debug, Clone)]
pub enum Op {
    /// Show `text` (already bidi-decorated). `last` is the letter that caused it, used to
    /// restart cleanly if a direct-insert preedit finds the caret moved (variant C3).
    Update {
        text: String,
        last: Option<char>,
    },
    Commit {
        text: String,
        append: String,
    },
    /// Variant B: finish with `text`, then open a new preedit showing `preedit`.
    CommitAndCompose {
        text: String,
        preedit: String,
    },
    Cancel,
    Insert(String),
    ToggleZwnj,
}

#[implement(ITfEditSession)]
struct EditSession {
    service: ComObject<TextService>,
    context: ITfContext,
    op: Op,
    generation: u32,
    _tok: ObjToken,
}

impl ITfEditSession_Impl for EditSession_Impl {
    fn DoEditSession(&self, ec: u32) -> windows_core::Result<()> {
        guard(|| unsafe {
            // A queued Update from before the host terminated our composition must not
            // resurrect it with text the engine has already forgotten.
            if matches!(self.op, Op::Update { .. }) && self.generation != self.service.generation()
            {
                return Ok(());
            }
            apply(&self.service, &self.context, ec, &self.op)
        })
    }
}

/// Queue (or with `sync`, run now) an edit session. Returns the session's own HRESULT when it
/// ran synchronously, TF_S_ASYNC when queued.
pub fn request(
    service: ComObject<TextService>,
    context: &ITfContext,
    op: Op,
    sync: bool,
) -> windows_core::Result<HRESULT> {
    let client_id = service.client_id();
    let generation = service.generation();
    let session: ITfEditSession = EditSession {
        service,
        context: context.clone(),
        op,
        generation,
        _tok: ObjToken::new(),
    }
    .into();
    let flags = if sync {
        TF_ES_READWRITE | TF_ES_SYNC
    } else {
        // "Run now if the document can be locked, else queue."
        TF_ES_READWRITE | TF_ES_ASYNCDONTCARE
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
    op: &Op,
) -> windows_core::Result<()> {
    match op {
        Op::Update { text, last } => show(svc, ctx, ec, text, *last),
        Op::Commit { text, append } => {
            finish_any(svc, ctx, ec, text)?;
            if !append.is_empty() {
                insert(svc, ctx, ec, append)?;
            }
            Ok(())
        }
        Op::CommitAndCompose { text, preedit } => {
            finish_any(svc, ctx, ec, text)?;
            show(svc, ctx, ec, preedit, None)
        }
        Op::Cancel => finish_any(svc, ctx, ec, ""),
        Op::Insert(text) => insert(svc, ctx, ec, text),
        Op::ToggleZwnj => {
            // The one deliberate read-back (PLAN 2.2): look at the char before an empty
            // selection; if anything about the read fails, fall back to inserting.
            if let Some(prev) = char_before_caret(ctx, ec) {
                if prev.1 == ZWNJ as u16 {
                    return prev.0.SetText(ec, 0, &[]);
                }
            }
            insert(svc, ctx, ec, "\u{200C}")
        }
    }
}

/// Display the preedit: in a TSF composition (default), or as plain document text that we
/// keep replacing (variant C3 `NoComposition`, the web keyboard's insert-then-fix-up model).
unsafe fn show(
    svc: &ComObject<TextService>,
    ctx: &ITfContext,
    ec: u32,
    text: &str,
    last: Option<char>,
) -> windows_core::Result<()> {
    let opts = svc.options();
    let range = if opts.no_composition {
        match svc.direct() {
            Some(r) if caret_at_end(ctx, ec, &r) => r,
            stale => {
                let mut text = text.to_string();
                if stale.is_some() {
                    // The user clicked/moved elsewhere: the old word is final as it stands;
                    // start a new word with just this letter.
                    svc.set_direct(None);
                    if let Some(c) = last {
                        text = opts.display(&svc.restart_with(c));
                    }
                }
                let r = ctx.cast::<ITfInsertAtSelection>()?.InsertTextAtSelection(
                    ec,
                    INSERT_TEXT_AT_SELECTION_FLAGS(0),
                    &wide(&text),
                )?;
                svc.set_direct(Some(r.clone()));
                set_attribute(svc, ctx, ec, &r)?;
                return caret_to_end(ctx, ec, &r);
            }
        }
    } else {
        let comp = match svc.composition() {
            Some(c) => c,
            None => {
                let c = start_composition(svc, ctx, ec)?;
                svc.set_composition(Some(c.clone()));
                c
            }
        };
        comp.GetRange()?
    };
    range.SetText(ec, TF_ST_CORRECTION, &wide(text))?;
    set_attribute(svc, ctx, ec, &range)?;
    caret_to_end(ctx, ec, &range)
}

/// End whatever preedit is open (composition or direct range) with `text`.
unsafe fn finish_any(
    svc: &ComObject<TextService>,
    ctx: &ITfContext,
    ec: u32,
    text: &str,
) -> windows_core::Result<()> {
    if let Some(comp) = svc.composition() {
        svc.set_composition(None);
        finish(ctx, ec, &comp, text)?;
    }
    if let Some(range) = svc.direct() {
        svc.set_direct(None);
        range.SetText(ec, TF_ST_CORRECTION, &wide(text))?;
        if let Ok(prop) = ctx.GetProperty(&GUID_PROP_ATTRIBUTE) {
            let _ = prop.Clear(ec, &range);
        }
        caret_to_end(ctx, ec, &range)?;
    }
    Ok(())
}

/// True when the selection is an empty caret at the end of `range` (or cannot be read).
unsafe fn caret_at_end(ctx: &ITfContext, ec: u32, range: &ITfRange) -> bool {
    let mut sel = [TF_SELECTION::default()];
    let mut fetched = 0u32;
    if ctx
        .GetSelection(ec, TF_DEFAULT_SELECTION, &mut sel, &mut fetched)
        .is_err()
        || fetched != 1
    {
        return true;
    }
    let Some(caret) = ManuallyDrop::take(&mut sel[0].range) else {
        return true;
    };
    let empty = caret.IsEmpty(ec).map(|b| b.as_bool()).unwrap_or(true);
    let at_end = caret
        .IsEqualStart(ec, range, TF_ANCHOR_END)
        .map(|b| b.as_bool())
        .unwrap_or(true);
    empty && at_end
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
    // A host may refuse (S_OK + NULL composition). windows-rs turns that into
    // Err(Error::empty()), whose HRESULT is 0 == S_OK, so map it to a real failure.
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

unsafe fn set_attribute(
    svc: &ComObject<TextService>,
    ctx: &ITfContext,
    ec: u32,
    range: &ITfRange,
) -> windows_core::Result<()> {
    let atom = svc.input_atom();
    if atom == 0 || svc.options().no_attribute {
        return Ok(()); // TF_INVALID_GUIDATOM: no underline, but composing still works
    }
    let prop = ctx.GetProperty(&GUID_PROP_ATTRIBUTE)?;
    let v = VARIANT::from(atom as i32);
    prop.SetValue(ec, range, &v)
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
    let opts = svc.options();
    // Variant C1 `PlainInsert`: the space after a commit goes in as plain text, not as a
    // second one-shot composition (CUAS hosts may lay out the two differently).
    let attempt = if opts.plain_insert || opts.no_composition {
        Err(E_UNEXPECTED.into())
    } else {
        start_composition(svc, ctx, ec)
    };
    match attempt {
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
    // TF_SELECTION holds the range in ManuallyDrop (it is an ABI struct): release it by hand or
    // every key leaks a range.
    ManuallyDrop::drop(&mut sel[0].range);
    r
}

/// Returns (range covering the previous char, that UTF-16 unit) when the selection is empty.
unsafe fn char_before_caret(ctx: &ITfContext, ec: u32) -> Option<(ITfRange, u16)> {
    let mut sel = [TF_SELECTION::default()];
    let mut fetched = 0u32;
    ctx.GetSelection(ec, TF_DEFAULT_SELECTION, &mut sel, &mut fetched)
        .ok()?;
    let range = ManuallyDrop::take(&mut sel[0].range)?;
    if fetched != 1 || !range.IsEmpty(ec).ok()?.as_bool() {
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
