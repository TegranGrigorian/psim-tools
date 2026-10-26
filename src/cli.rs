use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use crate::analyze::{self, AnalyzeOptions};
use crate::generate::{self, GenerateOptions};
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
    /// Summarize engine results into the files the site reads
    Analyze {
        /// City folder name under the data directory, e.g. ann_arbor
        #[arg(long)]
        city: String,

        /// Scenario to summarize (baseline, redesign_v1, ...)
        #[arg(long, default_value = "baseline")]
        scenario: String,

        /// Also write comparison.json: this scenario against the named earlier one
        #[arg(long)]
        compare: Option<String>,

        /// Folder that holds one subfolder per city
        #[arg(long, default_value = "data")]
        data_dir: PathBuf,
    },
    /// Generate simulated personas and their weekly trips
    Generate {
        /// City folder name under the data directory, e.g. ann_arbor
        #[arg(long)]
        city: String,

        /// Scenario name from the city's config (baseline, redesign_v1, ...)
        #[arg(long, default_value = "baseline")]
        scenario: String,

        /// Base seed. If omitted: config.json's base_seed, else derived from the clock.
        /// The seed actually used is printed and written to manifest.json.
        #[arg(long)]
        base_seed: Option<u64>,

        /// First replicate index (replicate i uses base_seed + i)
        #[arg(long, default_value_t = 0)]
        replicate: u64,

        /// Number of consecutive replicates to generate
        #[arg(long, default_value_t = 1)]
        replicates: u64,

        /// Folder that holds one subfolder per city
        #[arg(long, default_value = "data")]
        data_dir: PathBuf,

        /// Write runs here instead of <data-dir>/<city>/generated
        #[arg(long)]
        out_dir: Option<PathBuf>,
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
    fn analyze(opts: &AnalyzeOptions) -> Result<()> {
        let report = analyze::run(opts)?;
        println!(
            "{}: summarized {} replicate(s) -> {}",
            opts.scenario,
            report.replicates,
            report.out_dir.display()
        );
        for (dimension, value) in &report.skipped {
            println!("  left out {dimension}={value}: a metric had no data in any replicate");
        }
        println!("wrote {} file(s)", report.files.len());
        Ok(())
    }

    fn generate(opts: &GenerateOptions, verbose: bool) -> Result<()> {
        let report = generate::run(opts)?;
        println!(
            "base seed {} (from {:?}); replicate seeds {}..={}",
            report.base_seed,
            report.seed_source,
            report.runs.first().map_or(0, |r| r.seed),
            report.runs.last().map_or(0, |r| r.seed),
        );
        for run in &report.runs {
            if verbose || report.runs.len() == 1 {
                println!(
                    "{}: {} personas, {} trips -> {}",
                    run.run_id,
                    run.n_personas,
                    run.n_trips,
                    run.dir.display()
                );
            }
        }
        println!("generated {} run(s)", report.runs.len());
        Ok(())
    }

    fn test(test_json_parse: bool) {
        if test_json_parse {
            TestJson::test_rwrw();
        } else {
            println!("No test selected, try --test_json_parse");
        }
    }


    // map commands to their respective functions, this will be dispatched in the main CLI handling logic
    pub fn capture_input(cli: &Cli) -> Result<()> {
        match &cli.command {
            Commands::Analyze { city, scenario, compare, data_dir } => {
                let opts = AnalyzeOptions {
                    data_dir: data_dir.clone(),
                    city: city.clone(),
                    scenario: scenario.clone(),
                    compare: compare.clone(),
                };
                Self::analyze(&opts)?;
            }
            Commands::Generate {
                city,
                scenario,
                base_seed,
                replicate,
                replicates,
                data_dir,
                out_dir,
            } => {
                let opts = GenerateOptions {
                    data_dir: data_dir.clone(),
                    city: city.clone(),
                    scenario: scenario.clone(),
                    base_seed: *base_seed,
                    replicate: *replicate,
                    replicates: *replicates,
                    out_dir: out_dir.clone(),
                };
                Self::generate(&opts, cli.verbose)?;
            }
            Commands::Test { test_json_parse } => {
                Self::test(*test_json_parse);
            }
        }
        Ok(())
    }
}