//! "Config changed" signal from the settings app to every running IME instance.
//!
//! # Contract (the TIP side implements the receiving half)
//!
//! - **Object**: a named, manual-reset Win32 event [`CONFIG_CHANGED_EVENT`]
//!   (`Local\PersianPhoneticIme.ConfigChanged`, the per-session namespace). Whichever side
//!   comes first creates it (`CreateEventExW` with `CREATE_EVENT_MANUAL_RESET`, which opens it
//!   if it already exists); nobody owns it.
//! - **Security**: the creator passes an explicit descriptor, never the default one. The DACL
//!   grants only `SYNCHRONIZE | EVENT_MODIFY_STATE` to Everyone (S-1-1-0), ALL APPLICATION
//!   PACKAGES (S-1-15-2-1) and ALL RESTRICTED APPLICATION PACKAGES (S-1-15-2-2); the SACL is a
//!   low mandatory label with no-write-up. Otherwise an elevated host that creates the event
//!   first gives it a DACL of Administrators + SYSTEM only, and the medium-integrity settings
//!   app (and every non-elevated host) can no longer open it for the rest of the session.
//!   Openers ask only for the rights they use (TIP: `SYNCHRONIZE`; sender: `SYNCHRONIZE |
//!   EVENT_MODIFY_STATE`), never `EVENT_ALL_ACCESS`. A stray signal is harmless (it is only a
//!   hint), so granting signal rights to everyone in the session is acceptable.
//! - **Sender** (this crate, after every save or reset): `SetEvent`, keep it signaled
//!   for [`PULSE_MS`], then `ResetEvent`. Holding it for a while (instead of
//!   `PulseEvent`, which is documented as unreliable) lets every waiting process see
//!   it. Hand edits in a text editor do not signal; see the last point.
//! - **Receiver** (TIP, once per process, never on the key path): wait on the
//!   event with a thread-pool wait (`RegisterWaitForSingleObject` with
//!   `WT_EXECUTEONLYONCE`, or `CreateThreadpoolWait`). On wake, re-read
//!   `config.toml` off the UI thread, swap the `Arc<ResolvedConfig>` and rewrite
//!   `config-errors.txt`; treat the wake as a hint ("check whether the file
//!   changed"), so a spurious or repeated wake is harmless. Re-arm the wait no
//!   sooner than [`MIN_REARM_MS`] after the wake, because the event may still be
//!   signaled; a second save inside that window is caught by the focus-change check.
//! - **Not delivered everywhere**: AppContainer (immersive/UWP) processes have their
//!   own object namespace, so `Local\` there is a different object. The TIP only tries to
//!   open (never create) the session object by its `Session\<id>\` name and otherwise relies
//!   on re-checking the file on focus change (`ITfThreadMgrEventSink::OnSetFocus`, comparing
//!   the file's last-write time and size), which also catches hand edits.

use std::io;

/// Name of the event (pass to `CreateEventExW`).
pub const CONFIG_CHANGED_EVENT: &str = r"Local\PersianPhoneticIme.ConfigChanged";
/// How long the sender keeps the event signaled, in milliseconds.
pub const PULSE_MS: u32 = 250;
/// Minimum delay before a receiver re-arms its wait after a wake, in milliseconds.
pub const MIN_REARM_MS: u32 = 1000;

/// Tells running IME instances that `config.toml` changed. Blocks for [`PULSE_MS`].
pub fn signal_config_changed() -> io::Result<()> {
    signal_named(CONFIG_CHANGED_EVENT, PULSE_MS)
}

#[cfg(windows)]
pub use win::{event_security_acls, signal_named, NamedEvent};

#[cfg(windows)]
mod win {
    use std::io;
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{
        CreateEventExW, OpenEventW, ResetEvent, SetEvent, WaitForSingleObject,
        CREATE_EVENT_MANUAL_RESET, SYNCHRONIZATION_ACCESS_RIGHTS,
    };

    fn io_err(e: windows::core::Error) -> io::Error {
        io::Error::other(e.message())
    }

    /// Builds the event's explicit security descriptor (see the module docs).
    pub(crate) mod security {
        use std::ffi::c_void;
        use windows::Win32::Security::{
            AddAccessAllowedAce, AddMandatoryAce, InitializeAcl, InitializeSecurityDescriptor,
            SetSecurityDescriptorDacl, SetSecurityDescriptorSacl, ACE_FLAGS, ACL, ACL_REVISION,
            PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR,
        };

        pub const SYNCHRONIZE: u32 = 0x0010_0000;
        pub const EVENT_MODIFY_STATE: u32 = 0x0002;
        const SYSTEM_MANDATORY_LABEL_NO_WRITE_UP: u32 = 1;

        // Binary SIDs as u32 words: [revision 1 | count << 8, 6-byte big-endian authority
        // (last byte in the top bits of word 1), sub-authorities...].
        pub const EVERYONE: [u32; 3] = [0x0101, 0x0100_0000, 0];
        pub const ALL_APP_PACKAGES: [u32; 4] = [0x0201, 0x0F00_0000, 2, 1];
        pub const ALL_RESTRICTED_APP_PACKAGES: [u32; 4] = [0x0201, 0x0F00_0000, 2, 2];
        pub const LOW_LABEL: [u32; 3] = [0x0101, 0x1000_0000, 0x1000];
        const GRANTEES: [&[u32]; 3] = [&EVERYONE, &ALL_APP_PACKAGES, &ALL_RESTRICTED_APP_PACKAGES];

        pub fn sid(s: &[u32]) -> PSID {
            PSID(s.as_ptr() as *mut c_void)
        }

        /// ACL header plus one ACE (header + mask + SID) per SID.
        fn acl_len(sids: &[&[u32]]) -> u32 {
            8 + sids.iter().map(|s| 8 + 4 * s.len() as u32).sum::<u32>()
        }

        #[repr(C, align(8))]
        struct AclBuf([u8; 128]);

        /// An absolute security descriptor and the ACLs it points to (boxed so they never move).
        pub struct EventSecurity {
            sd: Box<SECURITY_DESCRIPTOR>,
            dacl: Box<AclBuf>,
            sacl: Box<AclBuf>,
        }

        impl EventSecurity {
            pub fn new() -> windows::core::Result<EventSecurity> {
                let mut me = EventSecurity {
                    sd: Box::new(SECURITY_DESCRIPTOR::default()),
                    dacl: Box::new(AclBuf([0; 128])),
                    sacl: Box::new(AclBuf([0; 128])),
                };
                let dacl = me.dacl.0.as_mut_ptr() as *mut ACL;
                let sacl = me.sacl.0.as_mut_ptr() as *mut ACL;
                let psd = PSECURITY_DESCRIPTOR(&mut *me.sd as *mut SECURITY_DESCRIPTOR as _);
                // SAFETY: the buffers are large enough (acl_len <= 128), aligned, and live in
                // `me`; the SIDs are valid constants, copied into the ACEs.
                unsafe {
                    InitializeAcl(dacl, acl_len(&GRANTEES), ACL_REVISION)?;
                    for s in GRANTEES {
                        AddAccessAllowedAce(
                            dacl,
                            ACL_REVISION,
                            SYNCHRONIZE | EVENT_MODIFY_STATE,
                            sid(s),
                        )?;
                    }
                    InitializeAcl(sacl, acl_len(&[&LOW_LABEL]), ACL_REVISION)?;
                    AddMandatoryAce(
                        sacl,
                        ACL_REVISION,
                        ACE_FLAGS(0),
                        SYSTEM_MANDATORY_LABEL_NO_WRITE_UP,
                        sid(&LOW_LABEL),
                    )?;
                    InitializeSecurityDescriptor(psd, 1)?;
                    SetSecurityDescriptorDacl(psd, true, Some(dacl), false)?;
                    SetSecurityDescriptorSacl(psd, true, Some(sacl), false)?;
                }
                Ok(me)
            }

            /// Attributes pointing at this descriptor; valid while `self` lives.
            pub fn attributes(&self) -> SECURITY_ATTRIBUTES {
                SECURITY_ATTRIBUTES {
                    nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                    lpSecurityDescriptor: &*self.sd as *const SECURITY_DESCRIPTOR as *mut c_void,
                    bInheritHandle: false.into(),
                }
            }

            #[cfg(test)]
            pub fn descriptor(&self) -> PSECURITY_DESCRIPTOR {
                PSECURITY_DESCRIPTOR(&*self.sd as *const SECURITY_DESCRIPTOR as *mut c_void)
            }

            /// The DACL and SACL bytes (ACL header's `AclSize` long).
            pub fn acl_bytes(&self) -> (Vec<u8>, Vec<u8>) {
                fn bytes(b: &AclBuf) -> Vec<u8> {
                    let len = u16::from_le_bytes([b.0[2], b.0[3]]) as usize;
                    b.0[..len.min(b.0.len())].to_vec()
                }
                (bytes(&self.dacl), bytes(&self.sacl))
            }
        }
    }

    /// DACL and SACL bytes of the event's descriptor, so the TIP can test that it builds the
    /// same one. `None` if it cannot be built.
    #[doc(hidden)]
    pub fn event_security_acls() -> Option<(Vec<u8>, Vec<u8>)> {
        security::EventSecurity::new().ok().map(|s| s.acl_bytes())
    }

    /// Opens or creates manual-reset event `name` with `access` rights only, giving a new event
    /// the explicit descriptor. Falls back to plain open, then to default security, so a
    /// refused descriptor never stops signalling.
    pub(crate) fn open_or_create_event(name: &str, access: u32) -> windows::core::Result<HANDLE> {
        let name = HSTRING::from(name);
        if let Ok(sec) = security::EventSecurity::new() {
            let sa = sec.attributes();
            // SAFETY: plain FFI; `sa` and the descriptor it points to outlive the call.
            let r = unsafe { CreateEventExW(Some(&sa), &name, CREATE_EVENT_MANUAL_RESET, access) };
            if let Ok(h) = r {
                return Ok(h);
            }
        }
        // SAFETY: plain FFI; the name outlives the calls.
        unsafe { OpenEventW(SYNCHRONIZATION_ACCESS_RIGHTS(access), false, &name) }
            .or_else(|_| unsafe { CreateEventExW(None, &name, CREATE_EVENT_MANUAL_RESET, access) })
    }

    /// An open handle to a named manual-reset event (the protocol in the module docs).
    #[derive(Debug)]
    pub struct NamedEvent(HANDLE);

    // SAFETY: event handles may be used from any thread.
    unsafe impl Send for NamedEvent {}
    unsafe impl Sync for NamedEvent {}

    impl NamedEvent {
        /// Creates the event, or opens it if another process already did.
        pub fn open_or_create(name: &str) -> io::Result<NamedEvent> {
            let access = security::SYNCHRONIZE | security::EVENT_MODIFY_STATE;
            open_or_create_event(name, access)
                .map(NamedEvent)
                .map_err(io_err)
        }

        /// Sets the event.
        pub fn set(&self) -> io::Result<()> {
            // SAFETY: self.0 is a valid event handle until drop.
            unsafe { SetEvent(self.0) }.map_err(io_err)
        }

        /// Resets the event.
        pub fn reset(&self) -> io::Result<()> {
            // SAFETY: as above.
            unsafe { ResetEvent(self.0) }.map_err(io_err)
        }

        /// Waits up to `timeout_ms`; true if the event was signaled.
        pub fn wait(&self, timeout_ms: u32) -> bool {
            // SAFETY: as above.
            unsafe { WaitForSingleObject(self.0, timeout_ms) == WAIT_OBJECT_0 }
        }

        /// Set, hold for `hold_ms`, reset.
        pub fn pulse(&self, hold_ms: u32) -> io::Result<()> {
            self.set()?;
            std::thread::sleep(std::time::Duration::from_millis(hold_ms.into()));
            self.reset()
        }
    }

    impl Drop for NamedEvent {
        fn drop(&mut self) {
            // SAFETY: we own the handle.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    /// Opens or creates event `name` and pulses it for `hold_ms`.
    pub fn signal_named(name: &str, hold_ms: u32) -> io::Result<()> {
        NamedEvent::open_or_create(name)?.pulse(hold_ms)
    }
}

/// No IME runs on other platforms; signalling is a no-op.
#[cfg(not(windows))]
pub fn signal_named(_name: &str, _hold_ms: u32) -> io::Result<()> {
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    use super::win::security::{self, EventSecurity};
    use super::*;
    use std::ffi::c_void;
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        CreateRestrictedToken, DuplicateTokenEx, EqualSid, GetAce, GetKernelObjectSecurity,
        GetSecurityDescriptorDacl, GetSecurityDescriptorSacl, ImpersonateLoggedOnUser,
        IsValidSecurityDescriptor, RevertToSelf, SecurityImpersonation, SetTokenInformation,
        TokenIntegrityLevel, TokenPrimary, ACCESS_ALLOWED_ACE, ACL, CREATE_RESTRICTED_TOKEN_FLAGS,
        DACL_SECURITY_INFORMATION, LABEL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
        SID_AND_ATTRIBUTES, TOKEN_ALL_ACCESS, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE,
        TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{
        CreateEventW, GetCurrentProcess, OpenEventW, OpenProcessToken,
        SYNCHRONIZATION_ACCESS_RIGHTS,
    };

    const READ_CONTROL: u32 = 0x0002_0000;

    fn test_name(tag: &str) -> String {
        // Never the real event name, so tests cannot wake an installed IME.
        format!(
            r"Local\PersianPhoneticIme.Test.{tag}.{}",
            std::process::id()
        )
    }

    #[test]
    fn waiter_in_other_handle_sees_pulse() {
        let name = test_name("pulse");
        let receiver = NamedEvent::open_or_create(&name).unwrap();
        assert!(!receiver.wait(0), "starts non-signaled");
        let waiter = std::thread::spawn(move || receiver.wait(5000));
        std::thread::sleep(std::time::Duration::from_millis(50));
        signal_named(&name, 100).unwrap();
        assert!(waiter.join().unwrap());
    }

    #[test]
    fn event_is_reset_after_pulse() {
        let name = test_name("reset");
        let receiver = NamedEvent::open_or_create(&name).unwrap();
        signal_named(&name, 10).unwrap();
        assert!(!receiver.wait(0));
    }

    #[test]
    fn real_name_is_session_local() {
        assert!(CONFIG_CHANGED_EVENT.starts_with(r"Local\"));
    }

    #[test]
    fn descriptor_builds_and_is_valid() {
        let sec = EventSecurity::new().unwrap();
        // SAFETY: the descriptor lives in `sec`.
        assert!(unsafe { IsValidSecurityDescriptor(sec.descriptor()) }.as_bool());
        let (dacl, sacl) = sec.acl_bytes();
        assert_eq!(dacl.len(), 8 + (8 + 12) + 2 * (8 + 16));
        assert_eq!(sacl.len(), 8 + (8 + 12));
        assert_eq!(u16::from_le_bytes([dacl[4], dacl[5]]), 3, "three grantees");
        assert_eq!(u16::from_le_bytes([sacl[4], sacl[5]]), 1, "one label");
        assert_eq!(sacl[8], 0x11, "SYSTEM_MANDATORY_LABEL_ACE_TYPE");
    }

    /// Reads the created object's real DACL and label back from the kernel.
    #[test]
    fn created_event_keeps_explicit_dacl_and_label() {
        let name = test_name("sd");
        let h = super::win::open_or_create_event(
            &name,
            security::SYNCHRONIZE | security::EVENT_MODIFY_STATE | READ_CONTROL,
        )
        .unwrap();
        let mut buf = vec![0u64; 128];
        let mut needed = 0u32;
        let psd = PSECURITY_DESCRIPTOR(buf.as_mut_ptr() as *mut c_void);
        // SAFETY: buffer of 1 KiB, pointers to locals.
        unsafe {
            GetKernelObjectSecurity(
                h,
                (DACL_SECURITY_INFORMATION | LABEL_SECURITY_INFORMATION).0,
                Some(psd),
                (buf.len() * 8) as u32,
                &mut needed,
            )
            .unwrap();
            let (mut present, mut defaulted) = (false.into(), false.into());
            let mut dacl: *mut ACL = std::ptr::null_mut();
            GetSecurityDescriptorDacl(psd, &mut present, &mut dacl, &mut defaulted).unwrap();
            assert!(present.as_bool() && !dacl.is_null());
            assert_eq!((*dacl).AceCount, 3);
            let mut ace: *mut c_void = std::ptr::null_mut();
            GetAce(dacl, 0, &mut ace).unwrap();
            let ace = &*(ace as *const ACCESS_ALLOWED_ACE);
            assert_eq!(
                ace.Mask,
                security::SYNCHRONIZE | security::EVENT_MODIFY_STATE
            );
            let ace_sid = PSID(&ace.SidStart as *const u32 as *mut c_void);
            assert!(EqualSid(ace_sid, security::sid(&security::EVERYONE)).is_ok());
            let mut sacl: *mut ACL = std::ptr::null_mut();
            GetSecurityDescriptorSacl(psd, &mut present, &mut sacl, &mut defaulted).unwrap();
            assert!(present.as_bool() && !sacl.is_null());
            assert_eq!((*sacl).AceCount, 1);
            let _ = CloseHandle(h);
        }
    }

    /// A low-integrity token restricted to Everyone (so not matched by a default DACL, like a
    /// medium process facing an elevated creator's event) can open and signal ours.
    #[test]
    fn restricted_low_integrity_opener_can_open() {
        let ours = test_name("restricted");
        let theirs = test_name("restricted.default");
        let ours_h = NamedEvent::open_or_create(&ours).unwrap();
        // SAFETY: plain FFI.
        let theirs_h =
            unsafe { CreateEventW(None, true, false, &HSTRING::from(theirs.as_str())) }.unwrap();
        let (ok_ours, ok_theirs) = std::thread::spawn(move || {
            let access =
                SYNCHRONIZATION_ACCESS_RIGHTS(security::SYNCHRONIZE | security::EVENT_MODIFY_STATE);
            // SAFETY: plain FFI on locals; impersonation is undone before the thread ends.
            unsafe {
                let mut proc_tok = HANDLE::default();
                OpenProcessToken(
                    GetCurrentProcess(),
                    TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY,
                    &mut proc_tok,
                )
                .unwrap();
                // Lower the integrity on a duplicate first; the restricted token inherits it.
                let mut low = HANDLE::default();
                DuplicateTokenEx(
                    proc_tok,
                    TOKEN_ALL_ACCESS,
                    None,
                    SecurityImpersonation,
                    TokenPrimary,
                    &mut low,
                )
                .unwrap();
                let label = TOKEN_MANDATORY_LABEL {
                    Label: SID_AND_ATTRIBUTES {
                        Sid: security::sid(&security::LOW_LABEL),
                        Attributes: 0x20, // SE_GROUP_INTEGRITY
                    },
                };
                SetTokenInformation(
                    low,
                    TokenIntegrityLevel,
                    &label as *const TOKEN_MANDATORY_LABEL as *const c_void,
                    (std::mem::size_of::<TOKEN_MANDATORY_LABEL>() + 12) as u32,
                )
                .unwrap();
                let restrict = [SID_AND_ATTRIBUTES {
                    Sid: security::sid(&security::EVERYONE),
                    Attributes: 0,
                }];
                let mut tok = HANDLE::default();
                CreateRestrictedToken(
                    low,
                    CREATE_RESTRICTED_TOKEN_FLAGS(0),
                    None,
                    None,
                    Some(&restrict),
                    &mut tok,
                )
                .unwrap();
                let _ = CloseHandle(low);
                ImpersonateLoggedOnUser(tok).unwrap();
                let a = OpenEventW(access, false, &HSTRING::from(ours.as_str()));
                let b = OpenEventW(access, false, &HSTRING::from(theirs.as_str()));
                RevertToSelf().unwrap();
                for h in [a.as_ref().ok(), b.as_ref().ok()].into_iter().flatten() {
                    let _ = CloseHandle(*h);
                }
                let _ = CloseHandle(tok);
                let _ = CloseHandle(proc_tok);
                (a.is_ok(), b.is_ok())
            }
        })
        .join()
        .unwrap();
        assert!(
            ok_ours,
            "explicit descriptor admits the restricted low opener"
        );
        assert!(!ok_theirs, "control: default security does not");
        drop(ours_h);
        // SAFETY: we own the handle.
        unsafe {
            let _ = CloseHandle(theirs_h);
        }
    }
}
