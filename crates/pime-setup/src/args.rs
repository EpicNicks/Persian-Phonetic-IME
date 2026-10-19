//! Command-line parsing (pure). Switches accept `/x`, `-x` or `--x`, case-insensitively.

use std::fmt;
use std::path::PathBuf;

use crate::guids;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Load the TIP DLL and call its DllRegisterServer.
    Register(PathBuf),
    /// Load the TIP DLL and call its DllUnregisterServer.
    Unregister(PathBuf),
    /// InstallLayoutOrTip for the current user.
    EnableUser,
    /// InstallLayoutOrTip(..., ILOT_UNINSTALL) for the current user.
    DisableUser,
    /// Read-only report.
    Status,
    /// Delete (or schedule for deletion at reboot) old version folders under the product root.
    CleanupOld {
        root: PathBuf,
        keep_version: String,
    },
    Help,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub command: Command,
    /// No console output (Active Setup, MSI). The log file is still written.
    pub quiet: bool,
    /// Overrides for testing placeholder payloads (e.g. the spike DLLs); default = product GUIDs.
    pub clsid: u128,
    pub profile: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgError {
    NoCommand,
    UnknownSwitch(String),
    MissingValue(&'static str),
    BadGuid(&'static str),
    TwoCommands,
    Unexpected(String),
}

impl fmt::Display for ArgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArgError::NoCommand => write!(f, "no command given"),
            ArgError::UnknownSwitch(s) => write!(f, "unknown switch {s:?}"),
            ArgError::MissingValue(what) => write!(f, "{what} needs a value"),
            ArgError::BadGuid(what) => write!(f, "{what} is not a GUID like {{xxxxxxxx-xxxx-...}}"),
            ArgError::TwoCommands => write!(f, "give exactly one command"),
            ArgError::Unexpected(s) => write!(f, "unexpected argument {s:?}"),
        }
    }
}

pub const USAGE: &str = "\
pime-setup: Persian Phonetic IME setup helper

  pime-setup /register <tip.dll>     call DllRegisterServer of a TIP DLL of this helper's bitness
  pime-setup /unregister <tip.dll>   call DllUnregisterServer
  pime-setup /enable-user            add the IME to the current user's input methods
  pime-setup /disable-user           remove it from the current user's input methods
  pime-setup /status                 read-only report (safe anywhere)
  pime-setup /cleanup-old <root> <keep-version>
                                     remove old version folders (installer use)
Options:
  /quiet                             no console output
  --clsid {GUID} --profile {GUID}    override the product GUIDs (testing placeholder DLLs)

Exit codes: 0 ok, 2 usage, 3 bad DLL path/bitness, 4 LoadLibrary failed, 5 export missing,
6 DllRegisterServer/DllUnregisterServer failed, 7 InstallLayoutOrTip failed,
8 cleanup incomplete, 10 status: not (fully) registered, 1 other error.
Log: %TEMP%\\PersianPhoneticIme\\pime-setup.log
";

fn switch_name(arg: &str) -> Option<String> {
    // Paths that follow a command are consumed as values, so they never reach this function.
    let s = arg
        .strip_prefix("--")
        .or_else(|| arg.strip_prefix('-'))
        .or_else(|| arg.strip_prefix('/'))?;
    if s.is_empty() {
        None
    } else {
        Some(s.to_ascii_lowercase())
    }
}

fn set(c: Command, slot: &mut Option<Command>) -> Result<(), ArgError> {
    if slot.replace(c).is_some() {
        Err(ArgError::TwoCommands)
    } else {
        Ok(())
    }
}

/// Parses everything after argv[0].
pub fn parse<S: AsRef<str>>(args: &[S]) -> Result<Invocation, ArgError> {
    let mut command = None;
    let mut quiet = false;
    let mut clsid = guids::CLSID_TEXT_SERVICE;
    let mut profile = guids::GUID_PROFILE_FA_PHONETIC;
    let mut it = args.iter().map(|a| a.as_ref());

    while let Some(arg) = it.next() {
        let Some(name) = switch_name(arg) else {
            return Err(ArgError::Unexpected(arg.to_string()));
        };
        match name.as_str() {
            "register" | "unregister" => {
                let dll = it.next().ok_or(ArgError::MissingValue("the DLL path"))?;
                let dll = PathBuf::from(dll);
                let c = if name == "register" {
                    Command::Register(dll)
                } else {
                    Command::Unregister(dll)
                };
                set(c, &mut command)?;
            }
            "enable-user" => set(Command::EnableUser, &mut command)?,
            "disable-user" => set(Command::DisableUser, &mut command)?,
            "status" => set(Command::Status, &mut command)?,
            "cleanup-old" => {
                let root = it.next().ok_or(ArgError::MissingValue("cleanup-old"))?;
                let keep = it.next().ok_or(ArgError::MissingValue("cleanup-old"))?;
                let c = Command::CleanupOld {
                    root: PathBuf::from(root),
                    keep_version: keep.to_string(),
                };
                set(c, &mut command)?;
            }
            "help" | "?" | "h" => set(Command::Help, &mut command)?,
            "quiet" | "q" => quiet = true,
            "clsid" => {
                let v = it.next().ok_or(ArgError::MissingValue("--clsid"))?;
                clsid = guids::parse(v).ok_or(ArgError::BadGuid("--clsid"))?;
            }
            "profile" => {
                let v = it.next().ok_or(ArgError::MissingValue("--profile"))?;
                profile = guids::parse(v).ok_or(ArgError::BadGuid("--profile"))?;
            }
            _ => return Err(ArgError::UnknownSwitch(arg.to_string())),
        }
    }
    Ok(Invocation {
        command: command.ok_or(ArgError::NoCommand)?,
        quiet,
        clsid,
        profile,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(args: &[&str]) -> Command {
        parse(args).unwrap().command
    }

    #[test]
    fn commands() {
        assert_eq!(
            cmd(&["/register", r"C:\Program Files\X\a.dll"]),
            Command::Register(PathBuf::from(r"C:\Program Files\X\a.dll"))
        );
        assert_eq!(
            cmd(&["-UNREGISTER", "a.dll"]),
            Command::Unregister(PathBuf::from("a.dll"))
        );
        assert_eq!(cmd(&["--enable-user"]), Command::EnableUser);
        assert_eq!(cmd(&["/Disable-User"]), Command::DisableUser);
        assert_eq!(cmd(&["/status"]), Command::Status);
        assert_eq!(cmd(&["/?"]), Command::Help);
        assert_eq!(
            cmd(&[
                "/cleanup-old",
                r"C:\Program Files\Persian Phonetic IME\",
                "0.2.0"
            ]),
            Command::CleanupOld {
                root: PathBuf::from(r"C:\Program Files\Persian Phonetic IME\"),
                keep_version: "0.2.0".into()
            }
        );
    }

    #[test]
    fn options_and_defaults() {
        let inv = parse(&["/status"]).unwrap();
        assert!(!inv.quiet);
        assert_eq!(inv.clsid, guids::CLSID_TEXT_SERVICE);
        assert_eq!(inv.profile, guids::GUID_PROFILE_FA_PHONETIC);

        let inv = parse(&[
            "/quiet",
            "/enable-user",
            "--clsid",
            "{674850A2-4C3B-46FA-9B17-3CA5465FFA18}",
            "--profile",
            "da9a94b1-be61-4ff6-a706-7feaf1e62a33",
        ])
        .unwrap();
        assert!(inv.quiet);
        assert_eq!(inv.command, Command::EnableUser);
        assert_eq!(inv.clsid, 0x674850a2_4c3b_46fa_9b17_3ca5465ffa18);
        assert_eq!(inv.profile, 0xda9a94b1_be61_4ff6_a706_7feaf1e62a33);
    }

    #[test]
    fn errors() {
        let none: [&str; 0] = [];
        assert_eq!(parse(&none), Err(ArgError::NoCommand));
        assert_eq!(parse(&["/quiet"]), Err(ArgError::NoCommand));
        assert_eq!(
            parse(&["/register"]),
            Err(ArgError::MissingValue("the DLL path"))
        );
        assert_eq!(
            parse(&["/status", "/enable-user"]),
            Err(ArgError::TwoCommands)
        );
        assert_eq!(
            parse(&["/frobnicate"]),
            Err(ArgError::UnknownSwitch("/frobnicate".into()))
        );
        assert_eq!(
            parse(&["status"]),
            Err(ArgError::Unexpected("status".into()))
        );
        assert_eq!(
            parse(&["/status", "--clsid", "nope"]),
            Err(ArgError::BadGuid("--clsid"))
        );
        assert_eq!(
            parse(&["/cleanup-old", "x"]),
            Err(ArgError::MissingValue("cleanup-old"))
        );
        assert!(matches!(parse(&["/"]), Err(ArgError::Unexpected(_))));
    }
}
