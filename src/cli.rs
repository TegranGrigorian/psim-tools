use clap::{Parser, Subcommand};
use crate::tests::test_json::TestJson;

// CLI tool for the "Who Is the Bus For?" project
#[derive(Parser, Debug)]
#[command(name = "who-is-the-bus-for", version, about = "An efficient Rust CLI", long_about = None)]
pub struct Cli {
    /// Enable verbose logging
    #[arg(short, long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Commands,
}

// Commands enumeration and its specific arguments
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Analyze transit data
    Analyze {
        /// Path to the input data file
        #[arg(short, long, value_name = "FILE")]
        input: String,
    },
    /// Generate simulated personas
    Generate {
        /// Number of personas to generate
        #[arg(short, long)]
        count: usize,
    },
    /// Run built-in self tests
    Test {
        /// Read and write the seed in tools_config.json, then restore it
        #[arg(long = "test_json_parse")]
        test_json_parse: bool,
    }
}

pub struct CommandFunctions;

// Implement of the command functions like generate and stufffffff
impl CommandFunctions {

    // Functions
    fn analyze(input: &str) {
        println!("Analyzing input: {}", input);
    }

    fn generate(count: usize) {
        println!("Generating {} personas", count);
    }

    fn test(test_json_parse: bool) {
        if test_json_parse {
            TestJson::test_rwrw();
        } else {
            println!("No test selected, try --test_json_parse");
        }
    }


    // map commands to their respective functions, this will be dispatched in the main CLI handling logic
    pub fn capture_input(command: &Commands) {
        match command {
            Commands::Analyze { input } => {
                Self::analyze(input);
            }
            Commands::Generate { count } => {
                Self::generate(*count);
            }
            Commands::Test { test_json_parse } => {
                Self::test(*test_json_parse);
            }
        }
    }
}