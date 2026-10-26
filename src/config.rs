//! Two kinds of config:
//! - `Config`: tool-level settings (`tools_config.json`, repo root): seed override, data dir.
//! - `CityConfig`: per-city simulation settings (`data/<city>/config.json`, SCHEMA section 1).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{Datelike, NaiveDate, Weekday};
use serde::{Deserialize, Serialize};

use crate::schema::Group;

fn parse_date(text: &str, field: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .with_context(|| format!("{field} `{text}` is not a YYYY-MM-DD date"))
}

/// Tool-level settings. Missing fields fall back to defaults.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Config {
    #[serde(default = "default_seed")]
    pub seed: u32,
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,
}

fn default_seed() -> u32 {
    42
}

fn default_data_dir() -> PathBuf {
    PathBuf::from("data")
}

impl Default for Config {
    fn default() -> Self {
        Self {
            seed: default_seed(),
            data_dir: default_data_dir(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("reading tool config {}", path.display()))?;
        serde_json::from_str(&text)
            .with_context(|| format!("parsing tool config {}", path.display()))
    }
}

/// Per-city settings, mirrors SCHEMA section 1.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct CityConfig {
    pub city_slug: String,
    pub display_name: String,
    pub timezone: String,
    /// True while the input tables are invented placeholders. `analyze` then marks every
    /// summary as a preview (the site shows a banner) so sample numbers can't pass as results.
    #[serde(default)]
    pub sample_data: bool,
    /// `[min_lon, min_lat, max_lon, max_lat]`
    pub bbox: [f64; 4],
    pub population_size: u32,
    /// Real population the personas stand in for (used for persona weights).
    pub represented_population: u32,
    pub n_replicates: u32,
    /// `None` means: use the CLI flag, else derive from the clock (see `rng`).
    pub base_seed: Option<u64>,
    /// Scenario name -> GTFS folders, relative to the city folder.
    pub gtfs: BTreeMap<String, Vec<String>>,
    pub osm: String,
    /// `YYYY-MM-DD`, a school-in-session weekday (Mon-Fri). Routes mon..fri trips.
    pub service_date: String,
    /// `YYYY-MM-DD`, a Saturday in the same period. Routes sat trips.
    pub weekend_service_date: String,
    pub departure_window_minutes: u32,
    pub max_walk_minutes: u32,
    pub max_trip_minutes: u32,
    /// Group name -> share. Must sum to 1.0. Keys are checked against `Group`.
    pub group_shares: BTreeMap<String, f64>,
}

impl CityConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("reading city config {}", path.display()))?;
        let cfg: Self = serde_json::from_str(&text)
            .with_context(|| format!("parsing city config {}", path.display()))?;
        cfg.validate()
            .with_context(|| format!("validating city config {}", path.display()))?;
        Ok(cfg)
    }

    /// Loads `<data_dir>/<city_slug>/config.json`.
    pub fn load_city(data_dir: &Path, city_slug: &str) -> Result<Self> {
        Self::load(&data_dir.join(city_slug).join("config.json"))
    }

    pub fn validate(&self) -> Result<()> {
        if self.population_size == 0 {
            bail!("population_size must be > 0");
        }
        if self.represented_population < self.population_size {
            bail!("represented_population must be >= population_size");
        }
        if self.n_replicates == 0 {
            bail!("n_replicates must be > 0");
        }
        let [min_lon, min_lat, max_lon, max_lat] = self.bbox;
        if min_lon >= max_lon || min_lat >= max_lat {
            bail!("bbox must be [min_lon, min_lat, max_lon, max_lat] with min < max");
        }
        let weekday = parse_date(&self.service_date, "service_date")?.weekday();
        if matches!(weekday, Weekday::Sat | Weekday::Sun) {
            bail!("service_date {} must be a weekday, got {weekday}", self.service_date);
        }
        let weekend = parse_date(&self.weekend_service_date, "weekend_service_date")?.weekday();
        if weekend != Weekday::Sat {
            bail!(
                "weekend_service_date {} must be a Saturday, got {weekend}",
                self.weekend_service_date
            );
        }
        if !self.gtfs.contains_key("baseline") {
            bail!("gtfs must define a `baseline` scenario");
        }
        for key in self.group_shares.keys() {
            if !Group::ALL.iter().any(|g| g.name() == key) {
                bail!("group_shares has unknown group `{key}`");
            }
        }
        let total: f64 = self.group_shares.values().sum();
        if (total - 1.0).abs() > 1e-6 {
            bail!("group_shares must sum to 1.0, got {total}");
        }
        if self.group_shares.values().any(|s| *s < 0.0) {
            bail!("group_shares must not be negative");
        }
        Ok(())
    }

    /// Real people each persona represents.
    pub fn persona_weight(&self) -> f64 {
        f64::from(self.represented_population) / f64::from(self.population_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> CityConfig {
        let text = r#"{
            "city_slug": "x", "display_name": "X", "timezone": "America/Detroit",
            "bbox": [-83.85, 42.20, -83.62, 42.34],
            "population_size": 1000, "represented_population": 100000,
            "n_replicates": 100, "base_seed": null,
            "gtfs": { "baseline": ["raw/gtfs/a"] },
            "osm": "raw/osm.pbf", "service_date": "2026-11-10",
            "weekend_service_date": "2026-11-14",
            "departure_window_minutes": 30, "max_walk_minutes": 20, "max_trip_minutes": 120,
            "group_shares": { "undergraduate": 0.5, "retired": 0.5 }
        }"#;
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn valid_config_passes() {
        sample().validate().unwrap();
    }

    #[test]
    fn shares_not_summing_to_one_fail() {
        let mut c = sample();
        c.group_shares.insert("retired".into(), 0.6);
        assert!(c.validate().is_err());
    }

    #[test]
    fn weekend_service_date_must_be_saturday() {
        let mut c = sample();
        c.weekend_service_date = "2026-11-15".into(); // Sunday
        assert!(c.validate().is_err());
    }

    #[test]
    fn service_date_must_be_a_weekday() {
        let mut c = sample();
        c.service_date = "2026-11-14".into(); // Saturday
        assert!(c.validate().is_err());
        c.service_date = "not a date".into();
        assert!(c.validate().is_err());
    }

    #[test]
    fn unknown_group_name_fails() {
        let mut c = sample();
        c.group_shares.insert("commuter".into(), 0.0);
        assert!(c.validate().is_err());
    }

    #[test]
    fn inverted_bbox_fails() {
        let mut c = sample();
        c.bbox = [-83.62, 42.20, -83.85, 42.34];
        assert!(c.validate().is_err());
    }

    #[test]
    fn missing_baseline_gtfs_fails() {
        let mut c = sample();
        c.gtfs.clear();
        assert!(c.validate().is_err());
    }

    #[test]
    fn persona_weight_scales_to_population() {
        assert_eq!(sample().persona_weight(), 100.0);
    }

    #[test]
    fn null_base_seed_is_none() {
        assert_eq!(sample().base_seed, None);
    }

    #[test]
    fn population_smaller_than_sample_fails() {
        let mut c = sample();
        c.represented_population = 10;
        assert!(c.validate().is_err());
    }

    #[test]
    fn tool_config_defaults_apply() {
        let c: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(c, Config::default());
        let c: Config = serde_json::from_str(r#"{"seed": 67}"#).unwrap();
        assert_eq!(c.seed, 67);
    }
}
