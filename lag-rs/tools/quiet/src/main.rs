//! `quiet <command> [args...]`: runs a command with Windows crash dialogs disabled.
//!
//! The user plays games on this PC while agents build and test. A crashing test binary (access violation)
//! otherwise pops a Windows "Application Error" box over the game. SetErrorMode is inherited by child
//! processes, so wrapping `cargo` covers every test executable it spawns. Nothing system-wide is changed.
//! `quiet --print-mode` prints this process's error mode (used to verify inheritance).
use windows::Win32::System::Diagnostics::Debug::{
    GetErrorMode, SetErrorMode, SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SEM_NOOPENFILEERRORBOX,
};

fn main() {
    let mut args = std::env::args_os().skip(1);
    let Some(cmd) = args.next() else {
        eprintln!("usage: quiet <command> [args...] | quiet --print-mode");
        std::process::exit(2);
    };
    if cmd == "--print-mode" {
        println!("error mode {:#x}", unsafe { GetErrorMode() });
        return;
    }
    unsafe { SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX) };
    let status = std::process::Command::new(&cmd).args(args).status();
    match status {
        Ok(s) => std::process::exit(s.code().unwrap_or(1)),
        Err(e) => {
            eprintln!("quiet: cannot run {cmd:?}: {e}");
            std::process::exit(127);
        }
    }
}
