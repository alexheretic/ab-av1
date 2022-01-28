use anyhow::ensure;
use std::{
    io,
    process::{ExitStatus, Output},
};

pub fn ensure_success(name: &'static str, out: &Output) -> anyhow::Result<()> {
    ensure!(
        out.status.success(),
        "{name} exit code {:?}\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
    );
    Ok(())
}

/// Convert exit code result into simple result.
pub fn exit_ok(name: &'static str, done: io::Result<ExitStatus>) -> anyhow::Result<()> {
    let code = done?;
    ensure!(code.success(), "{name} exit code {:?}", code.code());
    Ok(())
}

/// Ok -> None, err -> Some(err)
pub fn exit_ok_option<T>(
    name: &'static str,
    done: io::Result<ExitStatus>,
) -> Option<anyhow::Result<T>> {
    match exit_ok(name, done) {
        Ok(_) => None,
        Err(err) => Some(Err(err)),
    }
}
