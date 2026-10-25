//! File reading/writing helpers shared by the subcommands.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};

use crate::schema::{CampusLocation, Housing, Poi, Workplace};

pub fn read_csv<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    let mut reader = csv::Reader::from_path(path)
        .with_context(|| format!("opening {}", path.display()))?;
    let mut rows = Vec::new();
    for (i, row) in reader.deserialize().enumerate() {
        // +2: one for the header, one for 1-based line numbers
        let row: T = row.with_context(|| format!("{} line {}", path.display(), i + 2))?;
        rows.push(row);
    }
    Ok(rows)
}

pub fn write_csv<T: Serialize>(path: &Path, rows: &[T]) -> Result<()> {
    let mut writer = csv::Writer::from_path(path)
        .with_context(|| format!("creating {}", path.display()))?;
    for row in rows {
        writer.serialize(row)?;
    }
    writer.flush()?;
    Ok(())
}

/// `sha256:<hex>` of a file's bytes, for `manifest.json`.
pub fn sha256_file(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let digest = Sha256::digest(&bytes);
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("sha256:{hex}"))
}

/// The input tables for one city.
#[derive(Debug)]
pub struct CityInputs {
    pub housing: Vec<Housing>,
    pub workplaces: Vec<Workplace>,
    pub campus: Vec<CampusLocation>,
    pub pois: Vec<Poi>,
}

impl CityInputs {
    /// Reads and validates `housing.csv`, `workplaces.csv`, `campus_locations.csv`, `pois.csv`
    /// from `<city_dir>`.
    pub fn load(city_dir: &Path) -> Result<Self> {
        let inputs = Self {
            housing: read_csv(&city_dir.join("housing.csv"))?,
            workplaces: read_csv(&city_dir.join("workplaces.csv"))?,
            campus: read_csv(&city_dir.join("campus_locations.csv"))?,
            pois: read_csv(&city_dir.join("pois.csv"))?,
        };
        inputs.validate()?;
        Ok(inputs)
    }

    /// Value checks that the types can't express (SCHEMA section 2).
    pub fn validate(&self) -> Result<()> {
        for h in &self.housing {
            if h.units == 0 {
                bail!("housing {}: units must be >= 1", h.housing_id);
            }
            if h.tract_geoid.len() != 11 || !h.tract_geoid.bytes().all(|b| b.is_ascii_digit()) {
                bail!("housing {}: tract_geoid must be 11 digits", h.housing_id);
            }
        }
        for w in &self.workplaces {
            if w.jobs == 0 {
                bail!("workplace {}: jobs must be >= 1", w.workplace_id);
            }
        }
        for c in &self.campus {
            if c.capacity_weight <= 0.0 {
                bail!("campus {}: capacity_weight must be > 0", c.campus_id);
            }
        }
        for p in &self.pois {
            if p.weight <= 0.0 {
                bail!("poi {}: weight must be > 0", p.poi_id);
            }
        }
        check_unique(self.pois.iter().map(|p| p.poi_id.as_str()), "poi_id")?;
        check_unique(self.housing.iter().map(|h| h.housing_id.as_str()), "housing_id")?;
        check_unique(self.workplaces.iter().map(|w| w.workplace_id.as_str()), "workplace_id")?;
        check_unique(self.campus.iter().map(|c| c.campus_id.as_str()), "campus_id")?;
        Ok(())
    }
}

fn check_unique<'a>(ids: impl Iterator<Item = &'a str>, column: &str) -> Result<()> {
    let mut seen = std::collections::BTreeSet::new();
    for id in ids {
        if !seen.insert(id) {
            bail!("duplicate {column}: {id}");
        }
    }
    Ok(())
}
