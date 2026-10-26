//! `psim-tools analyze`: results of many replicates -> the summary files the site reads.

pub mod aggregate;
pub mod geo;
pub mod json_parse;
pub mod results;
pub mod routes;
pub mod stats;
pub mod summaries;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::config::CityConfig;
use crate::io::read_csv;
use crate::schema::{Housing, Persona, SCHEMA_VERSION, Trip};
use aggregate::Replicate;

#[derive(Debug, Clone)]
pub struct AnalyzeOptions {
    pub data_dir: PathBuf,
    pub city: String,
    pub scenario: String,
    /// Scenario to compare against: writes `comparison.json` (from `compare` to `scenario`).
    pub compare: Option<String>,
}

#[derive(Debug)]
pub struct AnalyzeReport {
    pub out_dir: PathBuf,
    pub replicates: usize,
    pub files: Vec<PathBuf>,
    pub skipped: Vec<(String, String)>,
}

/// `<scenario>_sNNNN` -> NNNN
fn seed_of(run_id: &str, scenario: &str) -> Option<u64> {
    run_id.strip_prefix(scenario)?.strip_prefix("_s")?.parse().ok()
}

/// Aggregates every run of a scenario that has results, in seed order.
pub fn load_replicates(city_dir: &Path, scenario: &str) -> Result<Vec<Replicate>> {
    let results_root = city_dir.join("results");
    let mut runs: Vec<(u64, String)> = fs::read_dir(&results_root)
        .with_context(|| format!("reading {} (run psim-engine first)", results_root.display()))?
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|name| seed_of(&name, scenario).map(|seed| (seed, name)))
        .filter(|(_, name)| results_root.join(name).join("trip_results.parquet").exists())
        .collect();
    runs.sort();
    if runs.is_empty() {
        bail!("no results for scenario `{scenario}` under {}", results_root.display());
    }
    runs.iter()
        .map(|(seed, run_id)| {
            let generated = city_dir.join("generated").join(run_id);
            let personas: Vec<Persona> = read_csv(&generated.join("personas.csv"))?;
            let trips: Vec<Trip> = read_csv(&generated.join("trips.csv"))?;
            let rows = results::read_trip_results(&results_root.join(run_id).join("trip_results.parquet"))?;
            aggregate::aggregate(*seed, &rows, &personas, &trips)
                .with_context(|| format!("aggregating {run_id}"))
        })
        .collect()
}

fn write_json(path: &Path, value: &Value, pretty: bool) -> Result<()> {
    let text = if pretty { serde_json::to_string_pretty(value)? } else { serde_json::to_string(value)? };
    fs::write(path, text + "\n").with_context(|| format!("writing {}", path.display()))
}

/// Adds this city's scenarios to `<data_dir>/summaries_index.json`. An index left by the
/// site's fake-data generator is replaced, so the preview banner goes away.
fn update_index(data_dir: &Path, cfg: &CityConfig, scenario: &str) -> Result<PathBuf> {
    let path = data_dir.join("summaries_index.json");
    let mut cities: Vec<Value> = Vec::new();
    if let Ok(text) = fs::read_to_string(&path) {
        let old: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if old.get("fake") != Some(&Value::Bool(true)) {
            cities = old["cities"].as_array().cloned().unwrap_or_default();
        }
    }
    let mut scenarios: Vec<String> = Vec::new();
    if let Some(existing) = cities.iter().find(|c| c["slug"] == cfg.city_slug.as_str()) {
        scenarios = existing["scenarios"]
            .as_array()
            .map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect())
            .unwrap_or_default();
    }
    if !scenarios.iter().any(|s| s == scenario) {
        scenarios.push(scenario.to_string());
    }
    scenarios.sort_by_key(|s| (s != "baseline", s.clone()));
    cities.retain(|c| c["slug"] != cfg.city_slug.as_str());
    cities.push(json!({
        "slug": cfg.city_slug,
        "display_name": cfg.display_name,
        "bbox": cfg.bbox,
        "scenarios": scenarios,
        "sample_data": cfg.sample_data,
    }));
    cities.sort_by_key(|c| c["slug"].as_str().unwrap_or("").to_string());
    // The site shows its preview banner when `fake` is set: do so if any city has sample inputs.
    let fake = cities.iter().any(|c| c["sample_data"] == Value::Bool(true));
    write_json(&path, &json!({ "schema_version": SCHEMA_VERSION, "fake": fake, "cities": cities }), true)?;
    Ok(path)
}

pub fn run(opts: &AnalyzeOptions) -> Result<AnalyzeReport> {
    let city_dir = opts.data_dir.join(&opts.city);
    let cfg = CityConfig::load_city(&opts.data_dir, &opts.city)?;
    if !cfg.gtfs.contains_key(&opts.scenario) {
        bail!("unknown scenario `{}`; config defines {:?}", opts.scenario, cfg.gtfs.keys().collect::<Vec<_>>());
    }
    let housing: Vec<Housing> = read_csv(&city_dir.join("housing.csv"))?;

    let reps = load_replicates(&city_dir, &opts.scenario)?;
    let out_dir = city_dir.join("summaries").join(&opts.scenario);
    fs::create_dir_all(&out_dir)?;
    let mut files = Vec::new();
    let mut put = |name: &str, value: &Value, pretty: bool| -> Result<()> {
        let path = out_dir.join(name);
        write_json(&path, value, pretty)?;
        files.push(path);
        Ok(())
    };

    let (group_metrics, skipped) = summaries::group_metrics(&cfg, &opts.scenario, &reps);
    put("group_metrics.json", &group_metrics, true)?;
    put("time_of_day.json", &summaries::time_of_day(&cfg, &opts.scenario, &reps), true)?;
    put(
        "neighborhood_map.geojson",
        &summaries::neighborhood_map(&cfg, &opts.scenario, &reps, &housing),
        false,
    )?;

    // Routes: for a redesign, mark the routes the baseline doesn't have.
    let baseline_ids = if opts.scenario != "baseline" && cfg.gtfs.contains_key("baseline") {
        Some(routes::routes_geojson(&cfg, &city_dir, "baseline", None)?.1)
    } else {
        None
    };
    let (routes_json, _) = routes::routes_geojson(&cfg, &city_dir, &opts.scenario, baseline_ids.as_ref())?;
    put("routes.geojson", &routes_json, false)?;

    if let Some(from) = &opts.compare {
        let before = load_replicates(&city_dir, from)?;
        put(
            "comparison.json",
            &summaries::comparison(&cfg, from, &opts.scenario, &before, &reps),
            true,
        )?;
    }
    files.push(update_index(&opts.data_dir, &cfg, &opts.scenario)?);

    Ok(AnalyzeReport {
        out_dir,
        replicates: reps.len(),
        files,
        skipped: skipped.into_iter().collect(),
    })
}
