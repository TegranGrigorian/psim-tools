//! `psim-tools generate`: inputs + seed -> personas.csv, trips.csv, manifest.json.

pub mod personas;
pub mod trips;

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use crate::config::CityConfig;
use crate::io::{CityInputs, sha256_file, write_csv};
use crate::rng::{SeedSource, replicate_rng, replicate_seed, resolve_base_seed};
use crate::schema::{Manifest, Persona, SCHEMA_VERSION, Trip};

/// Everything `generate` needs, independent of how it was asked for (CLI, tests).
#[derive(Debug, Clone)]
pub struct GenerateOptions {
    pub data_dir: PathBuf,
    pub city: String,
    pub scenario: String,
    /// `None`: use `config.json`'s `base_seed`, else the clock.
    pub base_seed: Option<u64>,
    /// First replicate index to generate.
    pub replicate: u64,
    /// How many consecutive replicates to generate.
    pub replicates: u64,
    /// Overrides `<data_dir>/<city>/generated`.
    pub out_dir: Option<PathBuf>,
}

/// What one generated replicate looks like on disk.
#[derive(Debug, Clone)]
pub struct RunSummary {
    pub run_id: String,
    pub dir: PathBuf,
    pub seed: u64,
    pub n_personas: usize,
    pub n_trips: usize,
}

#[derive(Debug)]
pub struct GenerateReport {
    pub base_seed: u64,
    pub seed_source: SeedSource,
    pub runs: Vec<RunSummary>,
}

/// One replicate in memory: a pure function of (config, inputs, seed).
pub fn generate_replicate(
    cfg: &CityConfig,
    inputs: &CityInputs,
    base_seed: u64,
    replicate: u64,
) -> Result<(Vec<Persona>, Vec<Trip>)> {
    let mut rng = replicate_rng(base_seed, replicate);
    let personas = personas::generate_personas(cfg, inputs, &mut rng)?;
    let trips = trips::generate_trips(&personas, &inputs.pois, &mut rng)?;
    Ok((personas, trips))
}

pub fn run(opts: &GenerateOptions) -> Result<GenerateReport> {
    let city_dir = opts.data_dir.join(&opts.city);
    let cfg = CityConfig::load_city(&opts.data_dir, &opts.city)?;
    if cfg.city_slug != opts.city {
        bail!(
            "config city_slug `{}` does not match folder `{}`",
            cfg.city_slug,
            opts.city
        );
    }
    if !cfg.gtfs.contains_key(&opts.scenario) {
        let known: Vec<&str> = cfg.gtfs.keys().map(String::as_str).collect();
        bail!("unknown scenario `{}`; config defines {:?}", opts.scenario, known);
    }
    if opts.replicates == 0 {
        bail!("replicates must be >= 1");
    }
    let inputs = CityInputs::load(&city_dir)?;

    let input_hashes: BTreeMap<String, String> =
        ["config.json", "housing.csv", "workplaces.csv", "campus_locations.csv", "pois.csv"]
            .iter()
            .map(|name| Ok((name.to_string(), sha256_file(&city_dir.join(name))?)))
            .collect::<Result<_>>()?;

    let (base_seed, seed_source) = resolve_base_seed(opts.base_seed, &cfg);
    let out_root = opts
        .out_dir
        .clone()
        .unwrap_or_else(|| city_dir.join("generated"));

    let mut runs = Vec::new();
    for replicate in opts.replicate..opts.replicate + opts.replicates {
        let seed = replicate_seed(base_seed, replicate);
        let run_id = format!("{}_s{:04}", opts.scenario, seed);
        let dir = out_root.join(&run_id);
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

        let (personas, trips) = generate_replicate(&cfg, &inputs, base_seed, replicate)?;
        write_csv(&dir.join("personas.csv"), &personas)?;
        write_csv(&dir.join("trips.csv"), &trips)?;

        let manifest = Manifest {
            schema_version: SCHEMA_VERSION.to_string(),
            run_id: run_id.clone(),
            city_slug: cfg.city_slug.clone(),
            scenario: opts.scenario.clone(),
            base_seed,
            seed,
            replicate,
            n_personas: cfg.population_size,
            psim_tools_version: env!("CARGO_PKG_VERSION").to_string(),
            generated_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            input_hashes: input_hashes.clone(),
        };
        let json = serde_json::to_string_pretty(&manifest)? + "\n";
        fs::write(dir.join("manifest.json"), json)
            .with_context(|| format!("writing manifest in {}", dir.display()))?;

        runs.push(RunSummary {
            run_id,
            dir,
            seed,
            n_personas: personas.len(),
            n_trips: trips.len(),
        });
    }

    Ok(GenerateReport {
        base_seed,
        seed_source,
        runs,
    })
}
