//! `pime-settings`: the settings app and config checker (see `cli::USAGE`).
//!
//! A console-subsystem exe so `--check` output and exit codes work in scripts and
//! CI; when the window is opened from Explorer (a console was created just for
//! us), that console is released at once.

use pime_settings::backup::Timestamp;
use pime_settings::cli::{self, Command};
use pime_settings::paths::Paths;
use pime_settings::report::exit;
use std::io::Write;

#[cfg(windows)]
mod ui;

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let cmd = match cli::parse(std::env::args_os().skip(1)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("pime-settings: {e}\n\n{}", cli::USAGE);
            return exit::USAGE;
        }
    };
    if cmd == Command::Help {
        print!("{}", cli::USAGE);
        return exit::OK;
    }
    if cmd == Command::PrintDefault {
        return cli::run_print_default(&mut std::io::stdout().lock());
    }
    let Some(paths) = Paths::from_env() else {
        eprintln!("pime-settings: can't find your AppData folder (APPDATA is not set).");
        return exit::IO_ERROR;
    };
    let (mut out, mut err) = (std::io::stdout().lock(), std::io::stderr().lock());
    let code = match cmd {
        Command::Check(path) => cli::run_check(&paths, path, &mut out, &mut err),
        Command::WriteErrors => cli::run_write_errors(&paths, &mut out, &mut err),
        Command::Reset => {
            let code = cli::run_reset(&paths, Timestamp::now(), &mut out, &mut err);
            if code == exit::OK {
                notify_reload(&mut err);
            }
            code
        }
        Command::Open => open(&paths, &mut err),
        Command::Ui | Command::ShowErrors => window(&paths, cmd == Command::ShowErrors),
        Command::Help | Command::PrintDefault => unreachable!(),
    };
    let _ = out.flush();
    code
}

fn notify_reload(err: &mut dyn Write) {
    if let Err(e) = pime_settings::reload::signal_config_changed() {
        let _ = writeln!(
            err,
            "pime-settings: couldn't notify the IME ({e}); it reloads on the next focus change."
        );
    }
}

#[cfg(windows)]
fn open(paths: &Paths, err: &mut dyn Write) -> i32 {
    if let Err(e) = pime_settings::backup::ensure_exists(&paths.config_file) {
        let _ = writeln!(err, "Couldn't create {}: {e}", paths.config_file.display());
        return exit::IO_ERROR;
    }
    match ui::open_in_editor(&paths.config_file) {
        Ok(()) => exit::OK,
        Err(e) => {
            let _ = writeln!(err, "Couldn't open {}: {e}", paths.config_file.display());
            exit::IO_ERROR
        }
    }
}

#[cfg(windows)]
fn window(paths: &Paths, errors_only: bool) -> i32 {
    ui::release_own_console();
    ui::run(paths.clone(), errors_only)
}

#[cfg(not(windows))]
fn open(_paths: &Paths, err: &mut dyn Write) -> i32 {
    let _ = writeln!(err, "pime-settings: --open is only available on Windows.");
    exit::USAGE
}

#[cfg(not(windows))]
fn window(_paths: &Paths, _errors_only: bool) -> i32 {
    eprintln!("pime-settings: the settings window is only available on Windows.");
    exit::USAGE
}
