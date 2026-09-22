//! Console entry point for Snail's fixture, benchmark, import, and outbox commands on Windows.
//!
//! The release `snail.exe` uses the Windows GUI subsystem. This small console process keeps a
//! terminal attached, forwards every argument, waits, and returns the GUI binary's exit status.

use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("snail-cli: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<u8, Box<dyn std::error::Error>> {
    let current = std::env::current_exe()?;
    let app = current.with_file_name(if cfg!(target_os = "windows") {
        "snail.exe"
    } else {
        "snail"
    });
    if !app.is_file() {
        return Err(format!(
            "{} is missing; build or unpack both executables",
            app.display()
        )
        .into());
    }

    let status = Command::new(app)
        .args(std::env::args_os().skip(1))
        .status()?;
    Ok(status.code().unwrap_or(1).clamp(0, u8::MAX as i32) as u8)
}
