use anyhow::anyhow;
use clap::{IntoApp, Parser};
use clap_complete::Shell;
use std::str::FromStr;

/// Print shell completions.
#[derive(Parser)]

pub struct Args {
    /// Shell.
    #[clap(default_value = "bash")]
    shell: String,
}

pub fn print_completions(Args { shell }: Args) -> anyhow::Result<()> {
    clap_complete::generate(
        Shell::from_str(&shell).map_err(|e| anyhow!("Shell {e}"))?,
        &mut crate::Command::command(),
        "ab-av1",
        &mut std::io::stdout(),
    );
    Ok(())
}
