use clap::Parser;
use psim_tools::cli::{Cli, CommandFunctions};

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    CommandFunctions::capture_input(&cli)
}
