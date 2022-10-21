use clap::{CommandFactory, Parser};
use clap_complete::Shell;

/// Print shell completions.
#[derive(Parser)]
#[group(skip)]
pub struct Args {
    /// Shell.
    #[arg(value_enum, default_value_t = Shell::Bash)]
    shell: Shell,
}

pub fn print_completions(Args { shell }: Args) -> anyhow::Result<()> {
    clap_complete::generate(
        shell,
        &mut crate::Command::command(),
        "ab-av1",
        &mut std::io::stdout(),
    );
    Ok(())
}
