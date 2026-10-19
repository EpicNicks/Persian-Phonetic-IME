# WS1 spike: Rust TSF text service (findings)

Status: **code complete and verified as far as is possible without registering it. Runtime TSF
behaviour is not verified yet: that needs the VM run in section 6.** Date: 2026-10-17.
Owner: WS1. Code: `spike/` (disposable, excluded from the root workspace).

## 1. Recommendation

**Continue with the Rust + windows-rs shell (D1 stays "Rust"). This is provisional until the VM
run in section 6 passes.**

Reasons:
- The whole interface set (ITfTextInputProcessorEx, ITfKeyEventSink, ITfThreadMgrEventSink,
  ITfCompositionSink, ITfDisplayAttributeProvider, ITfEditSession, IClassFactory, the display
  attribute info/enum objects) compiled against windows-rs 0.62.2 in about one working session,
  with no binding gaps. Every TSF method and constant needed was present in
  `windows::Win32::UI::TextServices`.
- `#[implement]` handles one object with many TSF interfaces correctly. QueryInterface, COM
  identity, base-interface QI and the standalone `ITfDisplayAttributeProvider` creation path are
  all proven by in-process unit tests (section 3).
- The traps we hit (section 4.1) are small and known now. Each has a fix in the spike that
  WS4/WS5 can copy.
- At least one real, fully Rust windows-rs TSF IME exists and is in active development
  (azooKey-Windows, section 5). We would not be the first.
- Most of what can still go wrong in the VM is about TSF semantics: host quirks, CUAS, sync edit
  sessions, signing. A C++ SampleIME fork would hit the same problems, so a VM failure is not by
  itself an argument for plan B.

Switch to plan B (Rust core + C++ shell) only if the VM run shows a failure that is specific to
Rust or windows-rs. Examples: a crash inside windows-rs generated code, or a COM lifetime bug we
cannot express safely. Hitting several such problems that each take days would also justify the
switch. A host merely behaving oddly (the same oddity would happen in C++) is not a reason.

**Remaining TSF work, Rust shell** (one developer, given this spike; replaces PLAN 11's WS4/WS5
numbers if the VM run passes):
- WS4 skeleton: **3-4 d** (factory, exports, registration, kill switch, guard, key sink and focus
  sinks are already written and can be lifted).
- WS5 composition: **5-7 d** (edit-session layer exists; the work left is the full 2.2 key table,
  ITfTextEditSink/selection tracking, compartments incl. OPENCLOSE, the candidate hooks and
  per-host fixes).
- WS6 candidate UI: **6-9 d**. This is now the part most likely to be painful in Rust:
  DirectWrite, a popup window, UIA providers and ITfCandidateListUIElement all go through raw
  windows-rs with a lot of boilerplate. It is still not a blocker.
- Langbar/input-mode button: 1-2 d.

Plan B would save little on WS4/WS5 (SampleIME gives a head start, but the C ABI to `pime-core`
and a mixed build add their own cost). It would mainly help WS6 (SampleIME ships a candidate
window).

## 2. What was built

`spike/` holds the crate `pime_spike` (cdylib + rlib), with its own `Cargo.toml`, `Cargo.lock`
and `.cargo/config.toml`. It uses windows 0.62.2 and windows-core 0.62.2 (the current crates.io
release when the spike was written).

| File | Content |
|---|---|
| `src/lib.rs` | The four DLL exports, `guard` (catch_unwind -> E_FAIL), poison flag, quiet panic hook, object/lock counting for DllCanUnloadNow, in-process COM tests |
| `src/engine.rs` | Tiny hard-coded engine (sh->ش, kh, ch, gh, zh, singles, ZWNJ token, lone `c` placeholder), key classification, fixed US-QWERTY printable table, 12 unit tests. No Windows types |
| `src/service.rs` | `TextService`: one `#[implement]` object for ITfTextInputProcessorEx (+ base), ITfKeyEventSink, ITfThreadMgrEventSink, ITfCompositionSink, ITfDisplayAttributeProvider. Kill switch in ActivateEx, compartment check (KEYBOARD_DISABLED / EMPTYCONTEXT on the context), commit on focus change, key-up symmetry |
| `src/edit.rs` | `ITfEditSession` + the document operations: start/update/finish composition, display attribute, caret placement, plain insert as start->set->end, ZWNJ toggle with read-back |
| `src/display.rs` | ITfDisplayAttributeInfo (solid underline, TF_ATTR_INPUT) and IEnumTfDisplayAttributeInfo |
| `src/factory.rs` | IClassFactory |
| `src/registry.rs` | DllRegisterServer/DllUnregisterServer bodies (InprocServer32, RegisterProfile fa-IR 0x0429, 7 categories from PLAN 5.4), kill switch and spike-option reads |
| `src/guids.rs` | Spike-only CLSID/profile/attribute GUIDs, deliberately different from the future product GUIDs |
| `scripts/` | `build.ps1` (host-safe), plus VM-only scripts: `register.ps1`, `unregister.ps1`, `test.ps1`, `killswitch.ps1`, `sign.ps1`, and the Windows Sandbox config `spike.wsb` |

Key behaviour while composing: letters append to the buffer and the whole preedit is re-rendered
into one composition. Space commits and inserts a space. Backspace edits the Latin buffer, and
removing the last letter cancels. Escape cancels. Enter commits (eaten); with the `EnterPasses`
option it commits synchronously and the Enter is then passed through. Arrows, Tab and Ctrl/Alt
chords commit and pass through. Digits and punctuation commit and append the US character.
Shift+Space toggles a ZWNJ inside the buffer.

Idle behaviour: letters start a composition. Shift+Space toggles a ZWNJ before the caret (with a
read-back). Digits and punctuation pass through by default, or are eaten and inserted from the
US table with `EatPrintable`. Everything else passes through. Modifier keys pressed alone never
commit.

Spike switches (VM only) are DWORD values under `HKCU\Software\PersianPhoneticIme\Spike`:
`EnterPasses` and `EatPrintable`, read at each activation. The environment variable
`PIME_SPIKE_HKL_SUBSTITUTE=1`, set at regsvr32 time, registers the profile with
hklSubstitute 0x04090409. `register.ps1` sets all three. Bidi and composition experiments (`BidiMark`, `BidiKeep`, `SpaceInComposition`, `PlainInsert`, `NoAttribute`, `NoComposition`) are described in section 8 and are switched with `scriptsariant.ps1`.

## 3. Verified here vs. unverified until the VM run

Nothing was registered on the dev machine. `test.ps1 -Check Unregistered` on the host confirms
there is no spike CLSID, TIP key or language-list entry.

| Item | Status | Evidence |
|---|---|---|
| `cargo build --release` x86_64-pc-windows-msvc | **verified** | `target-ws1/x86_64-pc-windows-msvc/release/pime_spike.dll`, 317 KB |
| i686 target installed + `cargo build --release` i686-pc-windows-msvc | **verified** | `rustup target add i686-pc-windows-msvc` worked; DLL 268 KB |
| `cargo clippy --all-targets -- -D warnings` (x64), `cargo clippy --release --target i686... -D warnings` | **verified** clean | |
| `cargo fmt --check` | **verified** | |
| Exports | **verified** (dumpbin /exports) | x64 and x86 both export `DllCanUnloadNow`, `DllGetClassObject`, `DllRegisterServer`, `DllUnregisterServer`. On x86 they are undecorated names aliased to `_Dll...@N`, which is what regsvr32/COM expect |
| No CRT DLL dependency | **verified** (dumpbin /dependents) | Imports only advapi32, ole32, kernel32, user32, oleaut32, combase, api-ms-win-core-synch-l1-2-0, ntdll. No vcruntime140/ucrtbase/msvcp |
| Control Flow Guard | **verified** (dumpbin /headers) | Built with `-C control-flow-guard`; "Control Flow Guard" in DLL characteristics on both |
| Engine logic | **verified** (12 unit tests) | digraph fix-up, Backspace, Escape, Space/Enter modes, ZWNJ toggle, modifiers never commit, printable handling, `would_eat` agrees with `on_key` for every key in every state |
| One object answers every TSF interface; COM identity; base-interface QI | **verified** (in-process COM tests, no registration) | `com_tests::one_object_answers_every_tsf_interface` |
| Standalone ITfDisplayAttributeProvider via the class factory, attribute info, enumerator returns **S_FALSE** at the end at the ABI | **verified** | `com_tests::display_provider_created_standalone` (calls the raw vtable slot) |
| Wrong CLSID -> CLASS_E_CLASSNOTAVAILABLE; null thread manager / null context fail cleanly | **verified** | `com_tests` |
| Panic -> E_FAIL + poison | **verified** | `com_tests::guard_turns_panic_into_e_fail` |
| CLSID key format, DLL path discovery without DllMain | **verified** | `registry::tests` |
| Script syntax | **verified** (PowerShell parser); VM guard refuses on this host | |
| DllRegisterServer / DllUnregisterServer actually work | **unverified (VM)** | compiled, never executed |
| TSF activates the TIP; keys arrive; composition + underline render | **unverified (VM)** | |
| OnTestKeyDown=TRUE then OnKeyDown=FALSE really passes the key to the app (commit-and-pass) | **unverified (VM)** | documented behaviour, but host-dependent in practice |
| TF_ES_SYNC commit before Enter pass-through | **unverified (VM)** | |
| ZWNJ read-back in Notepad/Word/Edge | **unverified (VM)** | |
| Layout pass-through, EatPrintable, hklSubstitute | **unverified (VM)** | |
| InstallLayoutOrTip psz spelling | **verified in Sandbox (2026-10-17)** | plain `0429:{CLSID}{PROFILE}` worked first try |
| Immersive hosts with an unsigned vs self-signed TIP | **unverified (VM)** | |
| x86 DLL in a 32-bit host | **unverified (VM)** | built only |

## 4. Findings per WS1 question

### 4.1 windows-rs ergonomics for the PLAN 5.2 interface set

Overall it is good. You write `#[implement(I1, I2, ...)]` on a plain struct and
`impl I1_Impl for Foo_Impl { ... }` for each interface. The generated code does QueryInterface,
AddRef/Release and the vtables. Things worth knowing (all handled in the spike):

1. **Many interfaces on one object works.** `TextService` lists 5 interfaces. ITfTextInputProcessor
   is answered automatically because `ITfTextInputProcessorEx_Vtbl::matches` also matches the
   base IID. You still have to write `impl ITfTextInputProcessor_Impl` (it is a supertrait), but
   you should not list it in `#[implement]`. azooKey lists both, which is redundant.
2. **Getting "self" as an interface needs no stored self-pointer.** Inside an `_Impl` method,
   `self.to_interface::<ITfKeyEventSink>()` and `self.to_object()` (a counted
   `ComObject<TextService>`) work. azooKey stores `this: Option<ITfTextInputProcessor>` inside
   its own object, which is a reference cycle. We do not need that.
3. **Ref counting is automatic, with one exception: `TF_SELECTION`.** Its `range` is a
   `ManuallyDrop<Option<ITfRange>>` because it is an ABI struct. After `SetSelection` you must
   `ManuallyDrop::drop` it, and after `GetSelection` you must `ManuallyDrop::take` it. Otherwise
   one range leaks per key. azooKey leaks here. See `edit::caret_to_end` / `char_before_caret`.
4. **S_FALSE is invisible both ways.** Trait methods return `Result<()>`, and Ok maps to S_OK.
   Enumerators must return S_FALSE at the end, or TSF can loop forever on them. The only way out
   is `Err(Error::from_hresult(S_FALSE))`, which the shim returns as-is. On the caller side,
   windows-rs wrappers turn S_FALSE into `Ok(())`, so you call the raw vtable when the
   distinction matters. The spike's test does that.
5. **`Error::empty()` is S_OK at the ABI.** windows-rs reports a NULL out-interface (for example
   StartComposition with a NULL composition, meaning "the host refused") as
   `Err(Error::empty())`. Its `.code()` is 0. Propagate that with `?` out of a COM method and the
   caller sees **S_OK**. Map it to a real error first (`edit::start_composition`).
6. **Out-params on error.** For `Result<BOOL>` methods (OnTestKeyDown...), returning `Err` leaves
   `*pfEaten` unwritten. TSF treats the call as failed, which is fine, but it means "not eaten"
   should be returned as `Ok(FALSE)`, not as an error.
7. **Re-entrancy.** A TF_ES_SYNC edit session, and some TSF callbacks, re-enter the same object on
   the same thread. Keep state in `RefCell` and **never hold a borrow across a TSF call**. The
   spike's handlers take what they need, drop the borrow, then call TSF. A stray double borrow
   would panic, and `guard` would turn that into E_FAIL plus poison, so a mistake degrades
   instead of crashing. Still, the WS4 design should keep this discipline explicit.
8. **Edit sessions as objects.** A small `#[implement(ITfEditSession)]` struct holding
   `ComObject<TextService>`, the context and an `Op` works well. Because of
   TF_ES_ASYNCDONTCARE the session may run after later keys, so (a) each `Op` carries the full
   text, not a delta, and (b) the session reads the current composition from the service when
   it runs. A generation counter drops stale Updates queued before a host termination.
9. **Hidden imports.** `TryFrom<&VARIANT> for i32` calls `VariantToInt32`, which imports
   **propsys.dll** into every host. The spike reads `vt`/`lVal` by hand. Keep an eye on
   dumpbin /dependents in CI.
10. **API shape changes between versions.** `BOOL` moved to `windows_core`, and interface params
    are `Ref<T>` (`.ok()?`, `.as_ref()`). azooKey (0.58) and the spike (0.62) differ in these
    details. Pin the exact version and upgrade deliberately.
11. **Unsafe surface.** About 50 `unsafe` blocks or functions across roughly 1 400 lines of shell code (including tests),
    all at COM call sites. Expected, and contained.

### 4.2 catch_unwind at every entry

- **windows-rs does not catch panics.** The generated `extern "system"` vtable shims call your
  `_Impl` method directly, and windows-core has no `catch_unwind` anywhere. Since Rust 1.81, a
  panic unwinding out of an `extern "system"` function aborts the process, so the host dies.
  Every `_Impl` method and DLL export therefore wraps its body in `guard(|| ...)`
  (`lib.rs`). The guard catches the panic, returns E_FAIL and sets a process-wide `POISONED`
  flag. After that the service eats no keys and ActivateEx advises nothing.
- `panic = "unwind"` is set in both profiles. Under `panic = "abort"`, catch_unwind is useless.
- The default panic hook is replaced by a silent one, installed lazily rather than in DllMain.
  This avoids stderr writes in GUI hosts and any chance of printing typed text.
- Remaining limits: a panic *inside a Drop that runs during unwinding* still aborts, and so does
  stack overflow or a non-unwinding foreign exception. Keep Drop impls trivial. The generated
  IUnknown methods (QI/AddRef/Release) are not wrapped, but they do not panic.
- A test case found a real property of the design. `POISONED` is process-global, so two tests
  touching it raced and one became flaky; they are now serialised. In the product this is the
  intended behaviour: one panic disables the TIP for that process.
- For WS4: a `#[guarded]` attribute macro (like azooKey's `#[macros::anyhow]`, but with
  catch_unwind) would remove the boilerplate. azooKey's macro converts anyhow errors to E_FAIL
  but does **not** catch panics, as far as the code we read shows.

### 4.3 i686 build

`rustup target add i686-pc-windows-msvc` worked, and the i686 release build, clippy and dumpbin
checks pass (section 3). The x86 DLL exports undecorated names. The static CRT and CFG flags in
`spike/.cargo/config.toml` apply to both targets. Note that `.cargo/config.toml` is only picked up
when cargo runs inside `spike/`, which is what `build.ps1` does. Not yet run inside a 32-bit
process.

### 4.4 Pass-through, layout and hklSubstitute

- Under fa-IR the underlying keyboard layout is Persian (00000429). Any printable key the TIP
  does not eat is translated by that layout, so `1` becomes `۱` and `,` becomes `،`, depending
  on the layout. This follows from how keyboard layouts work, and the VM run should confirm it.
- The docs do not explain what `hklSubstitute` does. RegisterProfile says only "the substitute
  hkl of this profile", and SubstituteKeyboardLayout says "sets a substitute keyboard layout for
  the specified language profile". Whether it makes the thread's active layout en-US while our
  profile is active, and whether a cross-language HKL (0x04090409 under 0x0429) is even
  accepted, has to be tested.
- **Recommended design (unchanged from PLAN 2.4):** eat every printable key without Ctrl/Alt and
  insert from our own fixed US-QWERTY table (`engine::us_printable`). This does not depend on
  hklSubstitute, so it is the primary design. hklSubstitute is a possible simplification, and
  we adopt it only if the VM proves it works on both Windows 10 and Windows 11.
- The spike can run all three variants: default (pass-through), `-EatPrintable`, and
  `-HklSubstitute`. Checklist item 6 records the results.
- Ctrl/Alt chords are never eaten, so shortcuts keep working. One thing to check in the VM:
  under the Persian layout, shortcuts are resolved by virtual key, so Ctrl+C still works.

### 4.5 TF_ES_SYNC for Enter pass-through

This is implemented as `Action::CommitAndPass`:
- OnTestKeyDown reports the key as eaten, so TSF calls OnKeyDown.
- OnKeyDown runs the commit in a `TF_ES_READWRITE | TF_ES_SYNC` edit session, then returns
  "not eaten" so the key reaches the app after our text is in the document.
- If the sync request is refused (the session HRESULT is TF_E_SYNCHRONOUS because the document is
  locked, or the request fails), the commit is queued async instead and the key is **eaten**.
  Losing one Enter is better than delivering it before the committed word.
- The same path is used for arrows, Tab and Ctrl/Alt chords while composing.
- Key-ups are eaten exactly when their key-down was, as SampleIME does, so the app never sees an
  unpaired WM_KEYUP.

Two things are unverified. First, whether every host honours "test said eaten, OnKeyDown said
not eaten": the TSF contract says yes, and CUAS hosts are the risk. Second, whether TF_ES_SYNC
is granted from inside a key event sink in Chromium and Word. Checklist items 4 and 5 cover
both.

### 4.6 Shift+Space ZWNJ read-back

The approach (`edit::char_before_caret`) does the read and the write **in the same edit
session**, so nothing can change between them even when the session runs async:
1. GetSelection(TF_DEFAULT_SELECTION) and require an empty selection.
2. Clone the range, ShiftStart(-1), and require that exactly -1 was moved.
3. GetText of 1 unit.
4. If that unit is U+200C, SetText("") on that range, which deletes it. Otherwise insert a
   ZWNJ. Any failure along the way also falls back to inserting (PLAN 2.2).

Inside a composition the toggle is pure engine state (pop the trailing ZWNJ token), so there is
no read-back there. Risks: hosts that do not expose text before the caret (some CUAS or
Chromium fields), and ShiftStart stopping at hidden or embedded content. In both cases we fall
back to inserting, which is safe.

### 4.7 InstallLayoutOrTip

- Export: `BOOL InstallLayoutOrTip(LPCWSTR psz, DWORD dwFlags)` in **input.dll**. There is no
  import library, so use LoadLibrary/GetProcAddress, or P/Invoke in script.
- Flags (not in public headers): ILOT_UNINSTALL 0x1 ("same as ILOT_DISABLED"), ILOT_DEFPROFILE
  0x2, ILOT_DEFUSER4 0x4, ILOT_NOAPPLYTOCURRENTSESSION 0x20, ILOT_CLEANINSTALL 0x40,
  ILOT_DISABLED 0x80.
- psz: `<LangID>:{CLSID}{ProfileGUID}`, with `;` between entries. The docs' own examples
  disagree on the LangID spelling (`0x0407:...` for layouts, `<LangID 1>:` in the TIP text).
  `register.ps1` tries `0429:` and then `0x0429:` and prints which one worked. Write that result
  down here.
- It is **per user** (it changes the user's input-method list). The installer must therefore
  call it in the user's context: an immediate, non-elevated custom action, or a first-run step
  of the settings app. A deferred custom action running as SYSTEM will not work for this. The
  call must happen after the machine-wide RegisterProfile.
- Microsoft's IME requirements page says to register with RegisterProfile, "don't write registry
  entries directly", and use InstallLayoutOrTip to make the IME usable immediately. Writing the
  default keyboard via the registry is discouraged.
- Fallback in the script: `Set-WinUserLanguageList` with `InputMethodTips` (`0429:{..}{..}`).

### 4.8 Clean unregister design

`DllUnregisterServer` (`registry::unregister`) is **best-effort and idempotent**. Every step runs
even if an earlier one failed, "already gone" counts as success, and the first real error is
returned. The order is the reverse of registration:
1. UnregisterCategory for each of the 7 categories.
2. UnregisterProfile(fa-IR, profile GUID).
3. `ITfInputProcessorProfiles::Unregister(clsid)`, which sweeps the whole
   `HKLM\...\CTF\TIP\{clsid}` key including stale profiles from older builds.
4. RegDeleteTree + RegDeleteKey of `HKLM\Software\Classes\CLSID\{clsid}`.

A failed DllRegisterServer calls unregister before returning, so a partial registration is
never left behind.

The full uninstall sequence (`unregister.ps1`, and the model for the WiX uninstall):
1. Per user, InstallLayoutOrTip(..., ILOT_UNINSTALL), while the profile still exists.
2. regsvr32 /u for the x86 DLL, then the x64 DLL. HKLM\Software\Classes is redirected per
   bitness, so each DLL removes its own CLSID view. The CTF TIP key is shared, so whichever runs
   first removes it and the second run is a no-op.
3. Delete files. A DLL loaded in a running process cannot be deleted, so the script renames it
   aside and asks for a reboot. The product uses versioned DLL names plus delete-on-reboot,
   as PLAN 5.4 says.
4. `test.ps1 -Check Unregistered` asserts the CLSID keys (both views), the TIP key, the profile
   and category keys and the user language-list entry are gone. It also reports whether a
   per-user `HKCU\...\CTF\TIP\{clsid}` key survives. TSF may create one, and nothing documented
   removes it, so the VM run should record whether it exists.

### 4.9 Immersive (UWP) apps and signing (S4)

- Microsoft's "Input Method Editors" page lists three requirements for third-party IMEs:
  "Must be digitally signed", "Must be TSF aware, with appropriate IME flags set correctly", and
  following the IME guidelines. It then says: "A third-party IME that doesn't meet these
  requirements is blocked from running."
- The requirements page says compatibility with Windows apps is declared by registering
  `GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT` (the spike does).
- The ime-rs README (a Rust port of SampleIME) states its DLL is "not digitally signed, so it
  isn't Windows Store-compatible". This matches the community understanding: unsigned TIPs load
  in desktop apps but not in immersive or AppContainer processes.
- **Conclusion:** on 19045, expect an unsigned TIP to be **ignored in immersive hosts**: Settings,
  Start/taskbar search (SearchApp.exe), Mail. That would also mean the WS1 "done when" criterion
  "Settings search" cannot pass unsigned.
- The docs do not say whether any valid Authenticode chain is enough, or whether a
  publicly trusted CA is required. `sign.ps1` tests a self-signed certificate trusted in
  LocalMachine\Root + TrustedPublisher inside the VM. If that works, development builds can be
  tested in immersive hosts. Either way, shipping needs real code signing (D8).
- Also from the requirements page: inside an AppContainer the TIP gets the app's capabilities, so
  there is no network and only limited file access. Data files must live under Program Files or
  Windows, or carry ACLs for ALL APPLICATION PACKAGES. `ITfThreadMgrEx::GetActiveFlags` with
  `TF_TMF_IMMERSIVEMODE` tells you that you are in one. Candidate windows must be owned windows
  (`ITfContextView::GetWnd`, falling back to GetFocus) and must raise EVENT_OBJECT_IME_SHOW,
  EVENT_OBJECT_IME_HIDE and EVENT_OBJECT_IME_CHANGE. These are inputs for WS6.

### 4.10 DPI and GetTextExt (from documentation; the spike has no candidate window)

- `ITfContextView::GetTextExt` returns the bounding box "in screen coordinates". It returns
  TS_E_NOLAYOUT when the layout is not computed yet, and S_OK with {0,0,0,0} when the text is not
  visible or the window is minimized. WS6 must treat an empty rect like TS_E_NOLAYOUT and fall
  back.
- Microsoft's IME guidance: the IME runs inside every process, so it "shouldn't declare a DPI
  awareness level". It runs at the host's awareness and should still render correctly per
  monitor. The coordinates GetTextExt returns are therefore in the host thread's DPI context.
  If our popup uses the same thread context, they match without conversion. PLAN 5.3 says
  "Per-monitor V2 DPI on our thread". If WS6 switches the thread DPI context for its window, it
  must convert GetTextExt rects with `LogicalToPhysicalPointForPerMonitorDPI` for DPI-unaware
  hosts. The simpler option is to leave the thread context alone and scale by
  `GetDpiForWindow`. This is unverified and belongs in the WS6/WS9 100/150/200% matrix.

## 5. Prior art read

- **azooKey-Windows** (github.com/fkunn1326/azooKey-Windows; Japanese IME, MIT) is a **fully Rust
  TSF text service** on windows-rs 0.58: a cdylib with one `#[implement]` object carrying 11
  interfaces (IClassFactory, ITfTextInputProcessor(Ex), ITfKeyEventSink, ITfThreadMgrEventSink,
  ITfTextLayoutSink, ITfCompositionSink, ITfDisplayAttributeProvider, ITfLangBarItem(Button),
  ITfSource). It uses edit sessions with closures and RefCell state, and its registration code
  looks like ours. It is the strongest evidence that the Rust shell is viable. Things not to
  copy: it stores a strong self-pointer, leaks the TF_SELECTION ranges, always returns S_FALSE
  from DllCanUnloadNow, has a logger thread spawned from DllMain, uses IPC to a Swift server on
  the key path, and has no catch_unwind in the code we read.
- **ime-rs** (github.com/saschanaz/ime-rs) is an incremental port of SampleIME to Rust. The C++
  shell (`cpp/SampleIME.sln`) is still present and Rust parts are linked in as a staticlib via
  cbindgen. In effect that is plan B in practice, and it shows plan B is workable. It is less
  useful as a pure-Rust reference.
- The **imekit** crate on docs.rs claims a TSF backend. We did not inspect it.
- **SampleIME** (Windows-classic-samples/Samples/IME/cpp/SampleIME): the spike follows its
  patterns. Edit sessions are requested with `TF_ES_ASYNCDONTCARE | TF_ES_READWRITE` from
  OnKeyDown. A composition starts with `InsertTextAtSelection(TF_IAS_QUERYONLY)` +
  `StartComposition`. Text is set with `SetText(TF_ST_CORRECTION)`. The caret is collapsed with
  SetSelection. The display attribute is cleared before EndComposition. The same `_IsKeyEaten`
  logic serves the test and real key-down and key-up calls. One caution: SampleIME's
  `_IsKeyboardDisabled` queries the KEYBOARD_DISABLED/EMPTYCONTEXT compartments on the *thread
  manager* after fetching the top context, and lets the second read overwrite the first. That
  looks like a bug. The spike checks both compartments on the context, which is where the docs
  put them.

## 6. VM test procedure

Never on the host: the scripts refuse unless they detect a VM or Sandbox, or `-Force` is given.

Preparation on the host:
1. `powershell -ExecutionPolicy Bypass -File spike\scripts\build.ps1` runs tests and clippy,
   builds x64 and x86, and stages `spike\dist\{x64,x86,scripts}`.

Windows Sandbox (fastest; Windows 10 19045; has Notepad, Edge, Settings; no Word):
2. Double-click `spike\dist\scripts\spike.wsb`. It maps `dist` read-only to `C:\pime`, with
   networking off.
3. In the sandbox, open an elevated PowerShell and run
   `powershell -ExecutionPolicy Bypass -File C:\pime\scripts\register.ps1`.
   The script prints its checks and the manual checklist. Win+Space selects
   "Persian Phonetic (spike)".
4. Work through the checklist (`test.ps1 -Checklist`). Re-run `register.ps1` with
   `-EnterPasses`, `-EatPrintable` or `-HklSubstitute` for the variants. Each run re-registers.
5. Run `C:\pime\scripts\sign.ps1` and retry Settings search and Start search.
6. Run `C:\pime\scripts\unregister.ps1`. Every check must report absent. Closing the sandbox
   discards everything.

Hyper-V VM (needed for Word/Outlook, the 32-bit host check, Windows 11, DPI 150/200%):
7. Take a checkpoint first. Copy `dist` in, then follow the same steps 3-6. For the x86 DLL, test
   a 32-bit host (for example 32-bit Office, or `C:\Windows\SysWOW64\notepad.exe` on Windows 10).
8. If typing breaks: Win+Space to another keyboard, `killswitch.ps1 -On`, or revert the
   checkpoint.

### VM results (fill in)

| # | Check | Win10 Sandbox | Win10 VM | Win11 VM | Notes |
|---|---|---|---|---|---|
| R | register.ps1 checks pass; which InstallLayoutOrTip spelling worked | pass; `0429:` | | | |
| 1 | Notepad: `sh` -> underlined ش, Space commits "ش " | ش works; **bidi placement problem, section 8** | | | browser search bar correct |
| 2 | Backspace / Escape | Escape clears | | | |
| 3 | ZWNJ in composition and idle toggle | works | | | |
| 4 | Left arrow commit-then-pass | | | | |
| 5 | Enter default / `-EnterPasses` (order correct?) | | | | |
| 6 | Idle digits: default / EatPrintable / HklSubstitute | | | | |
| 7 | Word, Edge (input/textarea/contenteditable), Settings search, Start search; unsigned vs sign.ps1 | | | | |
| 8 | Password field does not compose | | | | |
| 9 | Click elsewhere mid-composition | | | | |
| 10 | Kill switch | | | | |
| 11 | unregister.ps1 all absent; per-user CTF key left? | | | | |
| 12 | No crash events naming pime_spike.dll | | | | |
| 32 | x86 DLL in a 32-bit host | | | | |

Decision gate: rows 1, 2, 4, 5, 9, 10, 11, 12 must pass in Notepad and Edge, and on Word in the VM,
to confirm "Rust shell". Rows 6 and 7 are informational and feed WS5 and D8.

## 7. Notes for WS4/WS5 (lift from the spike)

- No DllMain. The module path comes from `GetModuleHandleExW(FROM_ADDRESS)`.
- `guard` everywhere (make it an attribute macro), plus process-wide poison, plus a quiet panic
  hook.
- Kill switch read once in ActivateEx. When disabled, return S_OK and advise nothing.
- One object for the TIP. RefCell state, never borrowed across TSF calls. Ops carry full text.
  Each session reads the live composition when it runs. A generation counter guards against
  host termination. Key-up symmetry.
- Insert even plain text through start->set->end composition, and fall back to
  InsertTextAtSelection if the host refuses a composition.
- Watch `dumpbin /dependents` for new system DLL imports (propsys via VARIANT helpers).
- Not covered by the spike, and still WS4/WS5 work: ITfTextEditSink (selection moved -> commit),
  ITfTextLayoutSink, ITfCompartmentEventSink with OPENCLOSE and the input-mode button,
  ITfThreadFocusSink, the candidate list UI element, ETW TraceLogging, an icon resource (the
  spike registers icon index 0 of a DLL that has no icons; the input indicator should then show
  the language tag), and real PLAN 5.4 GUIDs.

## 8. Follow-up: RTL preedit placement in classic Notepad (A/B variants)

**Sandbox observation (Windows 10, 2026-10-17).** In classic Notepad, in an LTR paragraph, the
underlined RTL word is drawn at the far right while it is being composed. On Space it jumps left
to its final position. The browser search bar behaves correctly. Google's Persian IME types the
next word where it will end up.

**Working hypothesis (unverified).** Classic Notepad is a Win32 EDIT control and is not
TSF-aware, so it runs through CUAS (Cicero Unaware Application Support). CUAS gives the TIP a
*transitory* document (`TS_SS_TRANSITORY`) that holds only the composition. The text is handed
to the app as an IMM32 composition string (GCS_COMPSTR) and, on commit, as a result string
(GCS_RESULTSTR). The EDIT control most likely lets the default IME UI draw the composition
string in its own composition window rather than laying it out inside its paragraph. That
window lays out an RTL-only string on its own, so the word appears right-aligned, and it only
joins the paragraph's bidi layout once it is committed.

If that is right:
- Bidi marks (A) change the layout inside that window. They may or may not move the word.
- Holding the space (B) mainly helps the *committed* text, because the trailing space no longer
  sits alone as a neutral at the end of an LTR paragraph.
- Only something that puts the text into the EDIT control's own buffer while typing (C3) would
  make it appear in its final place.

All of this is to be confirmed by the A/B run.

### Variants (registry `HKCU\Software\PersianPhoneticIme\Spike`, read at each activation)

| Id | Settings | What it does | Expected effect |
|---|---|---|---|
| 0 | (none) | Baseline | Bug reproduces |
| A1 | `BidiMark=rlm` | U+200F before and after the displayed composition, stripped at commit | May right-to-left-anchor the composition window text. Committed text is clean |
| A2 | `BidiMark=rlm`, `BidiKeep=1` | Same, RLMs kept in committed text | RLM after the word makes a following space resolve RTL in an LTR paragraph. Cost: invisible marks in the user's text |
| A3 | `BidiMark=rle` | U+202B ... U+202C (embedding), stripped | Explicit embedding, supported by Uniscribe in old controls |
| A4 | `BidiMark=rle`, `BidiKeep=1` | Same, kept | |
| A5 | `BidiMark=rli` | U+2067 ... U+2069 (isolate), stripped | Isolates are Unicode 6.3+. Old EDIT/Uniscribe may draw them as boxes or ignore them; this records what it does |
| B1 | `SpaceInComposition=1` | Space keeps "word + space" composing. The next letter commits it and starts a new composition. Any other key or a focus change commits | The space is never a lone trailing neutral |
| B2 | B1 + `BidiMark=rlm`, `BidiKeep=1` | | |
| C1 | `PlainInsert=1` | The space after a commit is inserted with InsertTextAtSelection, not as a second one-shot composition | Tests whether the two-composition commit causes the jump |
| C2 | `NoAttribute=1` | No underline display attribute | Tests whether CUAS treats attributed text differently |
| C3 | `NoComposition=1` | **No TSF composition.** The preedit is inserted as plain text and the same range is replaced on every key (the web keyboard's insert-then-fix-up model). If the caret has moved away from the range, the old word stays as it is and a new word starts with the current letter | The text is in the document, so it should appear in its final place. **Risk in CUAS hosts:** in a transitory document the inserted text may go straight to the app as a result string, so the next replacement might *append* instead of replace (doubled letters). That is exactly what the test must show |
| C4 | C3 + `BidiMark=rlm`, `BidiKeep=1` | | |

Engine semantics stay intact in every variant. The bidi controls are added by the shell around
the *displayed* text only (`Options::display` / `Options::committed`). Backspace, Escape and the
ZWNJ toggle work on the Latin buffer exactly as before. In B, Backspace after the held space
reopens the word for editing, and Escape commits the held word rather than deleting it.
Unit tests cover this: 23 tests, including B, decoration and stripping, and `would_eat`
agreement in the held state.

Other ideas from the docs, not implemented:
- `ITfContext::GetStatus` reports `TS_SS_TRANSITORY` for CUAS documents. The real TIP can use it
  to pick a per-host strategy, for example C3 only where it proves to work, without asking the
  user.
- In transitory documents the ZWNJ read-back cannot see the app's text, so it falls back to
  inserting, which is safe. The same applies to any later "read before caret" feature.
- Attribute spans inside the composition (ITfRange::SetText plus partial GUID_PROP_ATTRIBUTE)
  only change styling. There is no documented way to influence the CUAS composition window's
  alignment from a TIP.

### How to run it (Sandbox)

1. On the host, `spike\scripts\build.ps1 -SkipTests` (already done for this drop). In the
   sandbox, **close Notepad**, then re-run `C:\pime\scripts\register.ps1` from an elevated
   PowerShell so the new DLL is copied in. The mapped `C:\pime` updates live, but the installed
   copy under Program Files does not.
2. From a normal (non-elevated) PowerShell, run
   `powershell -ExecutionPolicy Bypass -File C:\pime\scripts\ab-test.ps1`, or
   `-Only A1,A3,B1,C3` for a quick pass. For each variant the script:
   - switches the options (`variant.ps1 -Name <id>`, HKCU only);
   - restarts Notepad and asks you to type `salam<Space>dust<Space>`, then Backspace,
     Escape and `mi<Shift+Space>x`;
   - records where the preedit was drawn, whether it jumped, and whether editing worked, in
     `Desktop\ab-results.txt`.
3. To set a single variant manually: `variant.ps1 -Name C3`, then reopen Notepad.
   `variant.ps1 -List` shows all variants and the current setting. Register-time flags
   (`register.ps1 -BidiMark rle -SpaceInComposition ...`) set the same values.
4. Re-check the browser search bar with the winning variant, so it does not regress where
   baseline was already correct.

Results (fill in from `ab-results.txt`):

| Id | Position while typing | Jump on Space | Editing OK | Notes |
|---|---|---|---|---|
| 0 | far right (reported) | yes | yes | |
| A1 | | | | |
| A2 | | | | |
| A3 | | | | |
| A4 | | | | |
| A5 | | | | |
| B1 | | | | |
| B2 | | | | |
| C1 | | | | |
| C2 | | | | |
| C3 | | | | |
| C4 | | | | |
