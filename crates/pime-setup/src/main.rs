//! pime-setup: register/unregister the TIP DLL, enable/disable it for the current user, report
//! status, clean up old version folders. Built for x64 and x86; each registers the DLL of its own
//! bitness. Run by the MSI (installer/Package.wxs); `/status` is read-only and safe anywhere.

#[cfg(not(windows))]
fn main() {
    eprintln!("pime-setup only runs on Windows");
    std::process::exit(pime_setup::exit::OTHER);
}

#[cfg(windows)]
fn main() {
    std::process::exit(app::run(std::env::args().skip(1).collect()));
}

#[cfg(windows)]
mod app {
    use std::io::Read;
    use std::path::Path;

    use pime_setup::args::{self, Command, Invocation};
    use pime_setup::cleanup;
    use pime_setup::exit;
    use pime_setup::guids::{self, braced};
    use pime_setup::log::Log;
    use pime_setup::pe;
    use pime_setup::profile::{self, LangSpelling};
    use pime_setup::sys::{self, CallError, Disposal, Root, View};

    struct Ctx {
        log: Log,
        quiet: bool,
    }

    impl Ctx {
        /// Console (unless quiet) and log.
        fn say(&self, msg: &str) {
            if !self.quiet {
                println!("{msg}");
            }
            self.log.line(msg);
        }
        fn fail(&self, code: i32, msg: &str) -> i32 {
            if !self.quiet {
                eprintln!("pime-setup: {msg}");
            }
            self.log.line(&format!("error (exit {code}): {msg}"));
            code
        }
    }

    pub fn run(argv: Vec<String>) -> i32 {
        let arch = pe::machine_name(pe::own_machine());
        let inv = match args::parse(&argv) {
            Ok(inv) => inv,
            Err(e) => {
                eprintln!("pime-setup: {e}\n\n{}", args::USAGE);
                return exit::USAGE;
            }
        };
        if inv.quiet {
            sys::hide_own_console();
        }
        let log = match inv.command {
            Command::Status | Command::Help => Log::none(),
            _ => Log::open(arch),
        };
        let ctx = Ctx {
            log,
            quiet: inv.quiet,
        };
        ctx.log.line(&format!(
            "start {:?} clsid={} profile={}",
            inv.command,
            braced(inv.clsid),
            braced(inv.profile)
        ));
        let code = match &inv.command {
            Command::Help => {
                println!("{}", args::USAGE);
                exit::OK
            }
            Command::Register(dll) => call_dll(&ctx, dll, c"DllRegisterServer"),
            Command::Unregister(dll) => call_dll(&ctx, dll, c"DllUnregisterServer"),
            Command::EnableUser => user_list(&ctx, &inv, true),
            Command::DisableUser => user_list(&ctx, &inv, false),
            Command::Status => status(&inv),
            Command::CleanupOld { root, keep_version } => cleanup_old(&ctx, root, keep_version),
        };
        ctx.log.line(&format!("exit {code}"));
        code
    }

    fn call_dll(ctx: &Ctx, dll: &Path, export: &std::ffi::CStr) -> i32 {
        let name = export.to_string_lossy();
        if !dll.is_absolute() {
            return ctx.fail(
                exit::BAD_DLL,
                &format!("{} must be an absolute path", dll.display()),
            );
        }
        let mut head = Vec::with_capacity(4096);
        let read = std::fs::File::open(dll).and_then(|f| f.take(4096).read_to_end(&mut head));
        if let Err(e) = read {
            return ctx.fail(
                exit::BAD_DLL,
                &format!("cannot read {}: {e}", dll.display()),
            );
        }
        if let Err(e) = pe::check_dll(&head, pe::own_machine()) {
            let why = match e {
                pe::PeError::NotPe => "not a PE file".to_string(),
                pe::PeError::NotDll => "not a DLL".to_string(),
                pe::PeError::WrongMachine { found, expected } => format!(
                    "is an {} DLL but this is the {} helper; use pime-setup.{}.exe",
                    pe::machine_name(found),
                    pe::machine_name(expected),
                    pe::machine_name(found)
                ),
            };
            return ctx.fail(exit::BAD_DLL, &format!("{}: {why}", dll.display()));
        }
        let _com = sys::Com::init();
        match sys::call_hresult_export(dll, export) {
            Ok(hr) if hr >= 0 => {
                ctx.say(&format!("{name} {} -> 0x{hr:08X}", dll.display()));
                exit::OK
            }
            Ok(hr) => ctx.fail(
                exit::DLL_CALL_FAILED,
                &format!("{name} {} failed: 0x{:08X}", dll.display(), hr as u32),
            ),
            Err(CallError::LoadFailed(e)) => ctx.fail(
                exit::LOAD_FAILED,
                &format!("LoadLibrary {} failed: error {e}", dll.display()),
            ),
            Err(CallError::MissingExport) => ctx.fail(
                exit::MISSING_EXPORT,
                &format!("{} has no export {name}", dll.display()),
            ),
        }
    }

    fn user_list(ctx: &Ctx, inv: &Invocation, enable: bool) -> i32 {
        let flags = if enable {
            profile::ILOT_ENABLE
        } else {
            profile::ILOT_UNINSTALL
        };
        let mut accepted: Option<LangSpelling> = None;
        for spelling in profile::SPELLINGS {
            let s = profile::tip_string(guids::LANGID_FA_IR, inv.clsid, inv.profile, spelling);
            match sys::install_layout_or_tip(&s, flags) {
                Ok(true) => {
                    ctx.say(&format!(
                        "InstallLayoutOrTip(\"{s}\", 0x{flags:X}) = TRUE ({spelling:?} spelling)"
                    ));
                    accepted = Some(spelling);
                    break;
                }
                Ok(false) => ctx
                    .log
                    .line(&format!("InstallLayoutOrTip(\"{s}\", 0x{flags:X}) = FALSE")),
                Err(CallError::LoadFailed(e)) => {
                    return ctx.fail(
                        exit::ILOT_FAILED,
                        &format!("cannot load input.dll: error {e}"),
                    )
                }
                Err(CallError::MissingExport) => {
                    return ctx.fail(
                        exit::ILOT_FAILED,
                        "input.dll has no InstallLayoutOrTip export",
                    )
                }
            }
        }
        let listed = in_user_list(inv);
        ctx.say(&format!(
            "current user's language list {} the entry",
            if listed {
                "contains"
            } else {
                "does not contain"
            }
        ));
        match accepted {
            Some(_) => exit::OK,
            None => ctx.fail(
                exit::ILOT_FAILED,
                "InstallLayoutOrTip returned FALSE for both spellings",
            ),
        }
    }

    fn in_user_list(inv: &Invocation) -> bool {
        sys::subkeys(Root::CurrentUser, View::Native, profile::USER_PROFILE_KEY)
            .iter()
            .any(|tag| {
                let key = format!(r"{}\{tag}", profile::USER_PROFILE_KEY);
                sys::value_names(Root::CurrentUser, View::Native, &key)
                    .iter()
                    .any(|v| profile::is_our_list_entry(v, inv.clsid, inv.profile))
            })
    }

    fn status(inv: &Invocation) -> i32 {
        let yes = |b: bool| if b { "yes" } else { "no" };
        println!("Persian Phonetic IME status (read-only)");
        println!("  CLSID    {}", braced(inv.clsid));
        println!("  Profile  {}", braced(inv.profile));
        let inproc = profile::inproc_key(inv.clsid);
        let mut x64 = false;
        for (label, view) in [("64-bit", View::Bits64), ("32-bit", View::Bits32)] {
            match sys::read_string(Root::LocalMachine, view, &inproc, None) {
                Some(path) => {
                    let exists = Path::new(&path).is_file();
                    let tm =
                        sys::read_string(Root::LocalMachine, view, &inproc, Some("ThreadingModel"));
                    println!(
                        "  COM server ({label} view): {path} (file exists: {}, ThreadingModel: {})",
                        yes(exists),
                        tm.as_deref().unwrap_or("missing")
                    );
                    x64 |= view == View::Bits64 && exists;
                }
                None => println!("  COM server ({label} view): not registered"),
            }
        }
        let tip = sys::key_exists(
            Root::LocalMachine,
            View::Bits64,
            &profile::tip_key(inv.clsid),
        );
        let prof_key = profile::language_profile_key(inv.clsid, guids::LANGID_FA_IR, inv.profile);
        let prof = sys::key_exists(Root::LocalMachine, View::Bits64, &prof_key);
        println!("  TSF TIP registered (HKLM): {}", yes(tip));
        println!("  fa-IR profile registered: {}", yes(prof));
        if prof {
            if let Some(d) = sys::read_string(
                Root::LocalMachine,
                View::Bits64,
                &prof_key,
                Some("Description"),
            ) {
                println!("    description: {d}");
            }
        }
        let user_tip = sys::key_exists(
            Root::CurrentUser,
            View::Native,
            &profile::tip_key(inv.clsid),
        );
        println!(
            "  per-user TSF state (HKCU CTF\\TIP key): {}",
            yes(user_tip)
        );
        println!(
            "  in current user's language list: {}",
            yes(in_user_list(inv))
        );
        for (label, root) in [("HKCU", Root::CurrentUser), ("HKLM", Root::LocalMachine)] {
            let v = sys::read_dword(
                root,
                View::Native,
                profile::KILL_SWITCH_KEY,
                profile::KILL_SWITCH_VALUE,
            );
            println!(
                "  kill switch {label}: {}",
                match v {
                    Some(0) | None => "off".to_string(),
                    Some(n) => format!("ON ({n})"),
                }
            );
        }
        if x64 && tip && prof {
            exit::OK
        } else {
            exit::NOT_REGISTERED
        }
    }

    fn cleanup_old(ctx: &Ctx, root: &Path, keep: &str) -> i32 {
        if !cleanup::is_product_root(root) {
            return ctx.fail(
                exit::USAGE,
                &format!("{} is not the product folder", root.display()),
            );
        }
        if !cleanup::is_version_name(keep) {
            return ctx.fail(exit::USAGE, &format!("{keep:?} is not a version"));
        }
        let Ok(entries) = std::fs::read_dir(root) else {
            ctx.say("product folder not found; nothing to clean");
            return exit::OK;
        };
        let names: Vec<String> = entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut failed = false;
        for dir_name in cleanup::stale_version_dirs(&name_refs, keep) {
            let dir = root.join(dir_name);
            let mut foreign = false;
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let file_name = entry.file_name().to_string_lossy().into_owned();
                let is_file = entry.file_type().is_ok_and(|t| t.is_file());
                if !is_file || !cleanup::is_our_file(&file_name) {
                    foreign = true;
                    continue;
                }
                let d = sys::remove_now_or_at_reboot(&entry.path(), false);
                ctx.say(&format!("{}: {d:?}", entry.path().display()));
                failed |= matches!(d, Disposal::Failed(_));
            }
            if foreign {
                ctx.say(&format!(
                    "{} kept: contains files this tool did not install",
                    dir.display()
                ));
                continue;
            }
            let d = sys::remove_now_or_at_reboot(&dir, true);
            ctx.say(&format!("{}: {d:?}", dir.display()));
            failed |= matches!(d, Disposal::Failed(_));
        }
        if failed {
            ctx.fail(
                exit::CLEANUP_INCOMPLETE,
                "some old files could not be removed or scheduled",
            )
        } else {
            exit::OK
        }
    }
}
