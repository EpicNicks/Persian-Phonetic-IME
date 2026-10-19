//! ITfKeyEventSink and key decoding (WM_KEYDOWN wparam/lparam + modifier state -> KeyEvent).

use pime_core::KeyEvent;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, VK_CAPITAL, VK_CONTROL, VK_MENU, VK_SHIFT,
};
use windows::Win32::UI::TextServices::{ITfContext, ITfKeyEventSink_Impl};
use windows_core::{Ref, BOOL, GUID};

use crate::exports::guard;
use crate::service::TextService_Impl;

/// Modifier state at the time the key message was generated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// Caps Lock toggled on (only the Latin mode cares; the Session folds case itself).
    pub caps: bool,
}

impl Modifiers {
    /// GetKeyState, not GetAsyncKeyState: it reflects the input queue at the time of this key
    /// message, which is what TSF's key sink is synchronised with.
    pub fn current() -> Self {
        let down = |vk: u16| unsafe { GetKeyState(vk as i32) } < 0;
        Modifiers {
            shift: down(VK_SHIFT.0),
            ctrl: down(VK_CONTROL.0),
            alt: down(VK_MENU.0),
            caps: unsafe { GetKeyState(VK_CAPITAL.0 as i32) } & 1 != 0,
        }
    }
}

/// A decoded key-down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decoded {
    pub ev: KeyEvent,
    pub caps: bool,
}

/// `wparam` is the virtual key; `lparam` the WM_KEYDOWN/WM_SYSKEYDOWN flags (bits 0-15 repeat
/// count, 16-23 scan code, 24 extended, 29 context code = Alt held, 30 previous state,
/// 31 transition). Returns `None` for values that are not a virtual key (never eat those).
pub fn decode_key(wparam: usize, lparam: isize, m: Modifiers) -> Option<Decoded> {
    if wparam == 0 || wparam > 0xFE {
        return None;
    }
    let lp = lparam as u32;
    if lp & (1 << 31) != 0 {
        return None; // a key-up delivered to the down path
    }
    let context_alt = lp & (1 << 29) != 0;
    Some(Decoded {
        ev: KeyEvent {
            vk: wparam as u32,
            shift: m.shift,
            ctrl: m.ctrl,
            alt: m.alt || context_alt,
        },
        caps: m.caps,
    })
}

pub(crate) fn decode_now(w: WPARAM, l: LPARAM) -> Option<Decoded> {
    decode_key(w.0, l.0, Modifiers::current())
}

/// Tracks virtual keys whose key-down we ate; their key-up is eaten too (as SampleIME does), so
/// the app never sees an unpaired WM_KEYUP.
#[derive(Default, Clone, Debug)]
pub struct KeySet([u64; 4]);
impl KeySet {
    pub fn set(&mut self, vk: usize, on: bool) {
        let vk = vk & 0xFF;
        let (i, b) = (vk / 64, 1u64 << (vk % 64));
        if on {
            self.0[i] |= b;
        } else {
            self.0[i] &= !b;
        }
    }
    pub fn get(&self, vk: usize) -> bool {
        let vk = vk & 0xFF;
        self.0[vk / 64] & (1u64 << (vk % 64)) != 0
    }
}

impl ITfKeyEventSink_Impl for TextService_Impl {
    fn OnSetFocus(&self, _foreground: BOOL) -> windows_core::Result<()> {
        guard(|| Ok(()))
    }

    fn OnTestKeyDown(
        &self,
        pic: Ref<ITfContext>,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> windows_core::Result<BOOL> {
        guard(|| {
            let Some(ctx) = pic.as_ref() else {
                return Ok(false.into());
            };
            let Some(key) = decode_now(wparam, lparam) else {
                return Ok(false.into());
            };
            Ok(self.test_key(ctx, &key).into())
        })
    }

    fn OnKeyDown(
        &self,
        pic: Ref<ITfContext>,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> windows_core::Result<BOOL> {
        guard(|| {
            let Some(ctx) = pic.as_ref() else {
                return Ok(false.into());
            };
            let Some(key) = decode_now(wparam, lparam) else {
                return Ok(false.into());
            };
            self.key_down(ctx, &key).map(Into::into)
        })
    }

    fn OnTestKeyUp(
        &self,
        _pic: Ref<ITfContext>,
        w: WPARAM,
        _l: LPARAM,
    ) -> windows_core::Result<BOOL> {
        guard(|| Ok(self.key_up(w.0, false).into()))
    }

    fn OnKeyUp(&self, _pic: Ref<ITfContext>, w: WPARAM, _l: LPARAM) -> windows_core::Result<BOOL> {
        guard(|| Ok(self.key_up(w.0, true).into()))
    }

    fn OnPreservedKey(&self, _pic: Ref<ITfContext>, g: *const GUID) -> windows_core::Result<BOOL> {
        // Only the Persian/Latin hotkey is ever preserved (config `toggle_hotkey`, unbound by
        // default).
        guard(|| {
            let ours = !g.is_null()
                && unsafe { *g } == GUID::from_u128(pime_ids::GUID_PRESERVED_KEY_TOGGLE);
            Ok((ours && self.toggle_open()).into())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pime_core::keys::*;

    const DOWN: isize = 0x001E_0001; // repeat 1, scan code 0x1E ('A')

    #[test]
    fn plain_and_modified_letters() {
        let d = decode_key(VK_A as usize, DOWN, Modifiers::default()).unwrap();
        assert_eq!(d.ev, KeyEvent::plain(VK_A));
        let m = Modifiers {
            shift: true,
            caps: true,
            ..Default::default()
        };
        let d = decode_key(VK_A as usize, DOWN, m).unwrap();
        assert_eq!(d.ev, KeyEvent::shifted(VK_A));
        assert!(d.caps);
        let m = Modifiers {
            ctrl: true,
            alt: true,
            ..Default::default()
        };
        let d = decode_key(VK_A as usize, DOWN, m).unwrap();
        assert!(d.ev.ctrl && d.ev.alt, "AltGr = Ctrl+Alt is a chord");
    }

    #[test]
    fn lparam_flags() {
        // WM_SYSKEYDOWN context code: Alt held even if GetKeyState missed it.
        let d = decode_key(VK_A as usize, DOWN | (1 << 29), Modifiers::default()).unwrap();
        assert!(d.ev.alt);
        // Auto-repeat (previous state bit) is still a key-down.
        assert!(decode_key(VK_BACK as usize, DOWN | (1 << 30), Modifiers::default()).is_some());
        // Extended keys (numpad Enter, right-hand arrows) decode to the same VK.
        let d = decode_key(VK_RETURN as usize, DOWN | (1 << 24), Modifiers::default()).unwrap();
        assert_eq!(d.ev.vk, VK_RETURN);
        // Transition bit set = key-up: never treat as a key-down.
        let up = (DOWN as u32 | (1u32 << 31) | (1 << 30)) as i32 as isize;
        assert!(decode_key(VK_A as usize, up, Modifiers::default()).is_none());
    }

    #[test]
    fn rejects_non_virtual_keys() {
        assert!(decode_key(0, DOWN, Modifiers::default()).is_none());
        assert!(decode_key(0xFF, DOWN, Modifiers::default()).is_none());
        assert!(decode_key(0x1_0041, DOWN, Modifiers::default()).is_none());
        // VK_PACKET (injected Unicode) is decoded; the Session commits and passes it through.
        assert!(decode_key(0xE7, DOWN, Modifiers::default()).is_some());
    }

    #[test]
    fn keyset() {
        let mut k = KeySet::default();
        k.set(VK_A as usize, true);
        k.set(0xFE, true);
        assert!(k.get(VK_A as usize) && k.get(0xFE) && !k.get(VK_A as usize + 1));
        k.set(VK_A as usize, false);
        assert!(!k.get(VK_A as usize));
    }
}
