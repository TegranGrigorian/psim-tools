//! `psim-tools prepare`: public data -> the input tables (`housing.csv`,
//! `workplaces.csv`, `campus_locations.csv`, `pois.csv`) and the derived values in
//! `config.json` (`group_shares`, `represented_population`, `undergraduate_dorm_share`).
//!
//! Everything is read from files `fetch.sh` downloaded, so a run is reproducible. Where
//! the data cannot settle something, the assumption lives in `prepare.json` (see
//! docs/DATA_SOURCES.md), not in this code.

pub mod census;
pub mod osm;
pub mod params;

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::io::write_csv;
use crate::schema::{
    BuildingType, CampusLocation, CampusZone, Group, Housing, IncomeBand, Poi, PoiCategory, Sector,
    ShiftPattern, Tenure, Workplace,
};
use census::{Block, PlaceAcs, Tract};
use osm::Feature;
use params::PrepareParams;

#[derive(Debug, Clone)]
pub struct PrepareOptions {
    pub data_dir: PathBuf,
    pub city: String,
}

#[derive(Debug)]
pub struct PrepareReport {
    pub lines: Vec<String>,
}

// ---------------------------------------------------------------------------
// Housing
// ---------------------------------------------------------------------------

fn income_band(tract: &Tract, p: &PrepareParams) -> Option<IncomeBand> {
    let income = tract.median_income?;
    Some(if income < p.assumptions.income_low_below {
        IncomeBand::Low
    } else if income >= p.assumptions.income_high_from {
        IncomeBand::High
    } else {
        IncomeBand::Mid
    })
}

fn r4(x: f64) -> f64 {
    (x * 1e4).round() / 1e4
}

/// Housing rows for one block: owner and renter households (the block's occupied units
/// split by its tract's renter share), student housing, and other group quarters.
pub fn housing_rows(block: &Block, tract: &Tract, city_renter_share: f64, p: &PrepareParams) -> Vec<Housing> {
    let tract6 = &block.tract[block.tract.len() - 6..];
    let make = |suffix: &str, tenure, building_type, units: u32, no_vehicle: Option<f64>| Housing {
        housing_id: format!("h{}{suffix}", block.geoid),
        lat: block.lat,
        lon: block.lon,
        tenure,
        building_type,
        units,
        neighborhood: format!("tract_{tract6}"),
        tract_geoid: block.tract.clone(),
        median_income_band: income_band(tract, p),
        share_18_24: tract.share_18_24.map(r4),
        no_vehicle_share: no_vehicle.map(r4),
    };

    let mut rows = Vec::new();
    let renters = (f64::from(block.occupied_units) * tract.renter_share.unwrap_or(city_renter_share)).round() as u32;
    let owners = block.occupied_units - renters.min(block.occupied_units);
    if owners > 0 {
        rows.push(make("o", Tenure::Own, BuildingType::SingleFamily, owners, tract.no_vehicle_owner));
    }
    if renters > 0 {
        // a block with many rental units is apartment buildings
        let kind = if renters >= 20 { BuildingType::LargeMulti } else { BuildingType::SmallMulti };
        rows.push(make("r", Tenure::Rent, kind, renters, tract.no_vehicle_renter));
    }
    if block.college_housing > 0 {
        rows.push(make("d", Tenure::Dorm, BuildingType::Dorm, block.college_housing, None));
    }
    if block.other_group_quarters > 0 {
        rows.push(make("g", Tenure::OtherGroup, BuildingType::Other, block.other_group_quarters, None));
    }
    rows
}

// ---------------------------------------------------------------------------
// Workplaces and campus
// ---------------------------------------------------------------------------

/// NAICS sector (`CNS01`..`CNS20`, 1-based) -> our sector. Education is split by whether
/// the block is on a university campus.
pub fn sector_of(cns: usize, on_university_land: bool) -> Sector {
    match cns {
        15 if on_university_land => Sector::University,
        15 => Sector::EducationK12,
        16 => Sector::Healthcare,
        7 | 18 => Sector::RetailFood,
        20 => Sector::Government,
        5 | 9 | 12 | 13 => Sector::IndustryTech,
        _ => Sector::OtherService,
    }
}

fn sector_key(s: Sector) -> String {
    crate::analyze::aggregate::enum_name(&s)
}

fn sector_abbrev(cns: usize) -> String {
    format!("s{cns:02}")
}

/// Workplace rows for one block. Jobs in a sector are split into a day row and a night row
/// by the sector's night share. Retail and food jobs in campus blocks are not counted as
/// the university's; everything else on university land is.
pub fn workplace_rows(
    block: &Block,
    jobs: &[u32; 20],
    on_university_land: bool,
    night_share: &BTreeMap<String, f64>,
) -> Vec<Workplace> {
    let mut rows = Vec::new();
    for (i, &n) in jobs.iter().enumerate() {
        if n == 0 {
            continue;
        }
        let cns = i + 1;
        let sector = sector_of(cns, on_university_land);
        let is_university = on_university_land && !matches!(cns, 7 | 18);
        let night = ((f64::from(n) * night_share.get(&sector_key(sector)).copied().unwrap_or(0.0)).round() as u32).min(n);
        for (count, shift, tag) in [
            (n - night, ShiftPattern::StandardDay, "d"),
            (night, ShiftPattern::Night, "n"),
        ] {
            if count > 0 {
                rows.push(Workplace {
                    workplace_id: format!("w{}_{}_{tag}", block.geoid, sector_abbrev(cns)),
                    lat: block.lat,
                    lon: block.lon,
                    name: None,
                    sector,
                    jobs: count,
                    shift_pattern: shift,
                    is_university,
                });
            }
        }
    }
    rows
}

// ---------------------------------------------------------------------------
// Points of interest
// ---------------------------------------------------------------------------

pub fn poi_rows(features: &[Feature], p: &PrepareParams, bbox: [f64; 4]) -> Vec<Poi> {
    let mut rows = Vec::new();
    for f in features {
        let (lon, lat) = f.geom.centroid();
        if lon < bbox[0] || lat < bbox[1] || lon > bbox[2] || lat > bbox[3] {
            continue;
        }
        let shop = f.tag("shop").and_then(|t| p.assumptions.poi_weights.shopping.get(t));
        let medical = [f.tag("amenity"), f.tag("healthcare")]
            .into_iter()
            .flatten()
            .find_map(|t| p.assumptions.poi_weights.medical.get(t));
        let (category, weight, prefix) = match (shop, medical) {
            (Some(&w), _) => (PoiCategory::Shopping, w, 's'),
            (None, Some(&w)) => (PoiCategory::Medical, w, 'm'),
            _ => continue,
        };
        rows.push(Poi {
            poi_id: format!("{prefix}{}", f.id),
            lat: (lat * 1e6).round() / 1e6,
            lon: (lon * 1e6).round() / 1e6,
            name: f.name(),
            category,
            weight,
        });
    }
    rows.sort_by(|a, b| a.poi_id.cmp(&b.poi_id));
    rows
}

// ---------------------------------------------------------------------------
// Group shares
// ---------------------------------------------------------------------------

/// Shares of adults 18+: students and retired from ACS, and the rest of working-age adults
/// split by where the jobs are (university standard-day, any non-standard shift, other
/// standard-day), so the shares match the workplace pools the generator samples from.
pub fn group_shares(acs: &PlaceAcs, workplaces: &[Workplace]) -> Result<BTreeMap<&'static str, f64>> {
    let working_age = acs.adults - acs.undergraduates - acs.graduates - acs.seniors;
    if working_age <= 0.0 {
        bail!("ACS adults ({}) are fewer than students plus seniors", acs.adults);
    }
    let sum = |keep: &dyn Fn(&Workplace) -> bool| -> f64 {
        workplaces.iter().filter(|w| keep(w)).map(|w| f64::from(w.jobs)).sum()
    };
    let staff = sum(&|w| w.is_university && w.shift_pattern == ShiftPattern::StandardDay);
    let shift = sum(&|w| w.shift_pattern != ShiftPattern::StandardDay);
    let resident = sum(&|w| !w.is_university && w.shift_pattern == ShiftPattern::StandardDay);
    let total = staff + shift + resident;
    if total <= 0.0 {
        bail!("no jobs to split the working-age share by");
    }
    let mut shares = BTreeMap::new();
    shares.insert("undergraduate", acs.undergraduates / acs.adults);
    shares.insert("graduate", acs.graduates / acs.adults);
    shares.insert("retired", acs.seniors / acs.adults);
    shares.insert("university_staff", working_age / acs.adults * staff / total);
    shares.insert("shift_worker", working_age / acs.adults * shift / total);
    shares.insert("resident_worker", working_age / acs.adults * resident / total);
    // round to 4 places, the largest absorbing the rounding so the total is exactly 1
    let mut rounded: BTreeMap<&'static str, f64> = shares.iter().map(|(k, v)| (*k, r4(*v))).collect();
    let error = 1.0 - rounded.values().sum::<f64>();
    let (&largest, _) = rounded.iter().max_by(|a, b| a.1.total_cmp(b.1)).unwrap();
    *rounded.get_mut(largest).unwrap() = r4(rounded[largest] + error);
    Ok(rounded)
}

// ---------------------------------------------------------------------------
// Orchestration
// ---------------------------------------------------------------------------

fn write_config(path: &Path, shares: &BTreeMap<&'static str, f64>, population: f64, dorm_share: f64) -> Result<()> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut cfg: Value = serde_json::from_str(&text)?; // keeps key order (serde_json preserve_order)
    let map = cfg.as_object_mut().context("config.json must be an object")?;
    let ordered: serde_json::Map<String, Value> =
        Group::ALL.iter().map(|g| (g.name().to_string(), json!(shares[g.name()]))).collect();
    map.insert("group_shares".into(), Value::Object(ordered));
    map.insert("represented_population".into(), json!(population.round() as u64));
    map.insert("undergraduate_dorm_share".into(), json!(r4(dorm_share)));
    fs::write(path, serde_json::to_string_pretty(&cfg)? + "\n").with_context(|| format!("writing {}", path.display()))
}

pub fn run(opts: &PrepareOptions) -> Result<PrepareReport> {
    let city_dir = opts.data_dir.join(&opts.city);
    let p = PrepareParams::load(&city_dir)?;
    let cfg = crate::config::CityConfig::load_city(&opts.data_dir, &opts.city)?;
    let mut log = Vec::new();

    let blocks = census::read_blocks(&city_dir.join(&p.pl_zip), &p.state, &p.county, &p.place)?;
    let tracts = census::read_tracts(&city_dir.join(&p.acs_tracts))?;
    let acs = census::read_place(&city_dir.join(&p.acs_place))?;
    let geoids: HashSet<&str> = blocks.iter().map(|b| b.geoid.as_str()).collect();
    let wac = census::read_wac(&city_dir.join(&p.wac_gz), &geoids)?;
    let features = osm::read_features(&city_dir.join(&p.osm_pois))?;
    log.push(format!(
        "{}: {} blocks, {} with jobs, {} tracts in ACS, {} OSM features",
        acs.name, blocks.len(), wac.len(), tracts.len(), features.len()
    ));

    // University land: polygons run by U-M or Michigan Medicine.
    let university: Vec<&Feature> = features
        .iter()
        .filter(|f| f.geom.is_polygon())
        .filter(|f| f.tag("operator").is_some_and(|o| p.university_operators.iter().any(|u| o.contains(u.as_str()))))
        .collect();
    let on_university_land = |b: &Block| university.iter().any(|f| f.geom.contains(b.lon, b.lat));
    log.push(format!("{} university polygons", university.len()));

    // Housing
    let mut housing = Vec::new();
    for b in &blocks {
        let tract = tracts.get(&b.tract).cloned().unwrap_or_default();
        housing.extend(housing_rows(b, &tract, acs.renter_share, &p));
    }
    housing.sort_by(|a, b| a.housing_id.cmp(&b.housing_id));
    let units = |t: Tenure| housing.iter().filter(|h| h.tenure == t).map(|h| u64::from(h.units)).sum::<u64>();
    let college: u64 = blocks.iter().map(|b| u64::from(b.college_housing)).sum();
    log.push(format!(
        "housing: {} rows; units own {} / rent {}; student housing {}; other group quarters {}",
        housing.len(), units(Tenure::Own), units(Tenure::Rent), units(Tenure::Dorm), units(Tenure::OtherGroup)
    ));

    // Workplaces
    let by_geoid: BTreeMap<&str, &Block> = blocks.iter().map(|b| (b.geoid.as_str(), b)).collect();
    let mut workplaces = Vec::new();
    for (geoid, jobs) in &wac {
        let b = by_geoid[geoid.as_str()];
        workplaces.extend(workplace_rows(b, jobs, on_university_land(b), &p.assumptions.night_share));
    }
    let job_total: u64 = workplaces.iter().map(|w| u64::from(w.jobs)).sum();
    let univ_jobs: u64 = workplaces.iter().filter(|w| w.is_university).map(|w| u64::from(w.jobs)).sum();
    let night_jobs: u64 = workplaces.iter().filter(|w| w.shift_pattern == ShiftPattern::Night).map(|w| u64::from(w.jobs)).sum();
    log.push(format!(
        "workplaces: {} rows, {} jobs; university {} (OBP reports 56,276 campus and hospital employees); night shift {}",
        workplaces.len(), job_total, univ_jobs, night_jobs
    ));

    // Campus locations: blocks with education jobs inside a named campus polygon.
    let mut campus = Vec::new();
    for (geoid, jobs) in &wac {
        let b = by_geoid[geoid.as_str()];
        let education = jobs[14]; // CNS15
        if education == 0 {
            continue;
        }
        let zone_of = p.campus_polygons.iter().find_map(|cp| {
            let poly = features.iter().find(|f| f.geom.is_polygon() && f.name().as_deref() == Some(cp.name.as_str()))?;
            poly.geom.contains(b.lon, b.lat).then(|| (cp.name.clone(), cp.zone))
        });
        if let Some((name, zone)) = zone_of {
            campus.push(CampusLocation {
                campus_id: format!("c{geoid}"),
                lat: b.lat,
                lon: b.lon,
                name: Some(name),
                campus_zone: zone,
                capacity_weight: f64::from(education),
            });
        }
    }
    if campus.is_empty() {
        bail!("no campus locations found: check campus_polygons names against the OSM data");
    }
    let by_zone = |z: CampusZone| campus.iter().filter(|c| c.campus_zone == z).map(|c| c.capacity_weight).sum::<f64>();
    log.push(format!(
        "campus: {} locations; education jobs central {} north {} athletic {} medical {} other {}",
        campus.len(), by_zone(CampusZone::Central), by_zone(CampusZone::North), by_zone(CampusZone::Athletic),
        by_zone(CampusZone::Medical), by_zone(CampusZone::Other)
    ));

    let pois = poi_rows(&features, &p, cfg.bbox);
    let n_cat = |c: PoiCategory| pois.iter().filter(|x| x.category == c).count();
    log.push(format!("pois: {} shopping, {} medical", n_cat(PoiCategory::Shopping), n_cat(PoiCategory::Medical)));
    if n_cat(PoiCategory::Shopping) == 0 || n_cat(PoiCategory::Medical) == 0 {
        bail!("need at least one shopping and one medical POI");
    }

    // Derived config values
    let shares = group_shares(&acs, &workplaces)?;
    let dorm_share = (college as f64 * p.assumptions.college_housing_undergraduate_fraction / acs.undergraduates).clamp(0.0, 1.0);
    log.push(format!(
        "group shares: {}",
        Group::ALL.iter().map(|g| format!("{} {:.1}%", g.name(), shares[g.name()] * 100.0)).collect::<Vec<_>>().join(", ")
    ));
    log.push(format!("undergraduate dorm share {:.1}% ({college} in student housing x {} / {} undergraduates)", dorm_share * 100.0, p.assumptions.college_housing_undergraduate_fraction, acs.undergraduates));

    write_csv(&city_dir.join("housing.csv"), &housing)?;
    write_csv(&city_dir.join("workplaces.csv"), &workplaces)?;
    write_csv(&city_dir.join("campus_locations.csv"), &campus)?;
    write_csv(&city_dir.join("pois.csv"), &pois)?;
    write_config(&city_dir.join("config.json"), &shares, acs.adults, dorm_share)?;
    log.push(format!(
        "wrote housing.csv, workplaces.csv, campus_locations.csv, pois.csv and config.json under {}",
        city_dir.display()
    ));
    if cfg.sample_data {
        log.push("config.json still has \"sample_data\": true. Review the numbers above, then set it to false.".to_string());
    }
    Ok(PrepareReport { lines: log })
}
