//! `data/<city>/prepare.json`: where the raw files are and the assumptions used.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
pub struct CampusPolygon {
    pub name: String,
    pub zone: crate::schema::CampusZone,
}

#[derive(Deserialize, Debug, Clone)]
pub struct PoiWeights {
    pub shopping: BTreeMap<String, f64>,
    pub medical: BTreeMap<String, f64>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Assumptions {
    /// Sector name (as in `Sector`) -> share of jobs on the night shift.
    pub night_share: BTreeMap<String, f64>,
    pub income_low_below: f64,
    pub income_high_from: f64,
    pub college_housing_undergraduate_fraction: f64,
    pub poi_weights: PoiWeights,
}

#[derive(Deserialize, Debug, Clone)]
pub struct PrepareParams {
    pub state: String,
    pub county: String,
    pub place: String,
    pub pl_zip: String,
    pub wac_gz: String,
    pub acs_tracts: String,
    pub acs_place: String,
    pub osm_pois: String,
    pub university_operators: Vec<String>,
    pub campus_polygons: Vec<CampusPolygon>,
    pub assumptions: Assumptions,
}

impl PrepareParams {
    pub fn load(city_dir: &Path) -> Result<Self> {
        let path = city_dir.join("prepare.json");
        let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }
}
