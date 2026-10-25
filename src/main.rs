use clap::Parser;
use psim_tools::cli::{Cli, CommandFunctions};

fn main() {
    let cli = Cli::parse();
    CommandFunctions::capture_input(&cli.command);
}
