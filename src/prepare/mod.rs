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
// University buildings
// ---------------------------------------------------------------------------

/// Splits `total` into whole numbers proportional to `weights`, summing exactly to `total`
/// (largest-remainder method).
pub fn distribute(total: u32, weights: &[f64]) -> Vec<u32> {
    let sum: f64 = weights.iter().sum();
    if weights.is_empty() || sum <= 0.0 {
        return vec![0; weights.len()];
    }
    let exact: Vec<f64> = weights.iter().map(|w| f64::from(total) * w / sum).collect();
    let mut parts: Vec<u32> = exact.iter().map(|x| x.floor() as u32).collect();
    let mut left = total - parts.iter().sum::<u32>();
    let mut order: Vec<usize> = (0..weights.len()).collect();
    order.sort_by(|&a, &b| (exact[b] - exact[b].floor()).total_cmp(&(exact[a] - exact[a].floor())).then(a.cmp(&b)));
    for i in order {
        if left == 0 {
            break;
        }
        parts[i] += 1;
        left -= 1;
    }
    parts
}

/// An academic or office building on a university campus.
#[derive(Debug, Clone)]
pub struct CampusBuilding {
    pub id: String,
    pub name: Option<String>,
    pub lon: f64,
    pub lat: f64,
    pub zone: CampusZone,
    pub area_m2: f64,
}

/// Buildings inside a named campus polygon, except those the hospital runs (the hospital and its
/// clinics are not classrooms), at least `min_m2` in footprint.
pub fn campus_buildings(buildings: &[Feature], polygons: &[(&Feature, CampusZone)], hospital: &[&Feature], p: &PrepareParams) -> Vec<CampusBuilding> {
    let mut out = Vec::new();
    for b in buildings.iter().filter(|b| b.geom.is_polygon()) {
        let area = b.geom.area_m2();
        if area < p.min_building_m2 {
            continue;
        }
        let (lon, lat) = b.geom.centroid();
        let run_by_hospital = b.tag("operator").is_some_and(|o| o.contains(p.hospital_operator.as_str()))
            || hospital.iter().any(|h| h.geom.contains(lon, lat));
        if run_by_hospital {
            continue;
        }
        if let Some((_, zone)) = polygons.iter().find(|(poly, _)| poly.geom.contains(lon, lat)) {
            out.push(CampusBuilding { id: b.id.clone(), name: b.name(), lon, lat, zone: *zone, area_m2: area });
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
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
    let buildings = osm::read_features(&city_dir.join(&p.osm_buildings))?;
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

    // Campus buildings: where classes are held and where university staff work.
    let named_polygon = |name: &str| features.iter().find(|f| f.geom.is_polygon() && f.name().as_deref() == Some(name));
    let polygons: Vec<(&Feature, CampusZone)> = p
        .campus_polygons
        .iter()
        .map(|cp| named_polygon(&cp.name).map(|f| (f, cp.zone)).with_context(|| format!("campus polygon `{}` not found in the OSM data", cp.name)))
        .collect::<Result<_>>()?;
    let hospital: Vec<&Feature> = features
        .iter()
        .filter(|f| f.geom.is_polygon() && f.tag("operator").is_some_and(|o| o.contains(p.hospital_operator.as_str())))
        .collect();
    let class_buildings = campus_buildings(&buildings, &polygons, &hospital, &p);
    if class_buildings.is_empty() {
        bail!("no campus buildings found inside the campus polygons");
    }
    let area_of = |z: CampusZone| class_buildings.iter().filter(|c| c.zone == z).map(|c| c.area_m2).sum::<f64>();
    log.push(format!(
        "campus: {} buildings, footprint m2 central {:.0} north {:.0} athletic {:.0} other {:.0} (hospital buildings excluded)",
        class_buildings.len(), area_of(CampusZone::Central), area_of(CampusZone::North), area_of(CampusZone::Athletic), area_of(CampusZone::Other)
    ));
    let areas: Vec<f64> = class_buildings.iter().map(|c| c.area_m2).collect();

    // Workplaces
    let by_geoid: BTreeMap<&str, &Block> = blocks.iter().map(|b| (b.geoid.as_str(), b)).collect();
    let mut workplaces = Vec::new();
    let mut relocated = 0u32;
    for (geoid, jobs) in &wac {
        let b = by_geoid[geoid.as_str()];
        let mut jobs = *jobs;
        if p.university_address_blocks.contains(geoid) {
            // LODES puts the university's jobs at its reported address; spread them over campus.
            relocated += std::mem::take(&mut jobs[14]); // CNS15
        }
        workplaces.extend(workplace_rows(b, &jobs, on_university_land(b), &p.assumptions.night_share));
    }
    let night = p.assumptions.night_share.get("university").copied().unwrap_or(0.0);
    for (b, n) in class_buildings.iter().zip(distribute(relocated, &areas)) {
        let night_jobs = ((f64::from(n) * night).round() as u32).min(n);
        for (count, shift, tag) in [(n - night_jobs, ShiftPattern::StandardDay, "d"), (night_jobs, ShiftPattern::Night, "n")] {
            if count > 0 {
                workplaces.push(Workplace {
                    workplace_id: format!("w{}_u_{tag}", b.id),
                    lat: (b.lat * 1e6).round() / 1e6,
                    lon: (b.lon * 1e6).round() / 1e6,
                    name: b.name.clone(),
                    sector: Sector::University,
                    jobs: count,
                    shift_pattern: shift,
                    is_university: true,
                });
            }
        }
    }
    workplaces.sort_by(|a, b| a.workplace_id.cmp(&b.workplace_id));
    let job_total: u64 = workplaces.iter().map(|w| u64::from(w.jobs)).sum();
    let univ_jobs: u64 = workplaces.iter().filter(|w| w.is_university).map(|w| u64::from(w.jobs)).sum();
    let night_jobs: u64 = workplaces.iter().filter(|w| w.shift_pattern == ShiftPattern::Night).map(|w| u64::from(w.jobs)).sum();
    log.push(format!(
        "workplaces: {} rows, {} jobs; university {} ({} moved from the LODES address block onto {} buildings; OBP reports 56,276 campus and hospital staff); night shift {}",
        workplaces.len(), job_total, univ_jobs, relocated, class_buildings.len(), night_jobs
    ));

    // Class locations: the same buildings, weighted by footprint.
    let campus: Vec<CampusLocation> = class_buildings
        .iter()
        .map(|c| CampusLocation {
            campus_id: format!("c{}", c.id),
            lat: (c.lat * 1e6).round() / 1e6,
            lon: (c.lon * 1e6).round() / 1e6,
            name: c.name.clone(),
            campus_zone: c.zone,
            capacity_weight: c.area_m2.round().max(1.0),
        })
        .collect();

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distribute_sums_exactly_and_follows_the_weights() {
        let parts = distribute(100, &[1.0, 1.0, 1.0]);
        assert_eq!(parts.iter().sum::<u32>(), 100);
        assert_eq!(parts, vec![34, 33, 33], "remainder goes to the first of equal fractions");
        assert_eq!(distribute(10, &[3.0, 1.0]), vec![8, 2]);
        assert_eq!(distribute(5, &[0.0, 0.0]), vec![0, 0]);
        assert_eq!(distribute(7, &[]), Vec::<u32>::new());
        assert_eq!(distribute(0, &[1.0, 2.0]), vec![0, 0]);
    }

    #[test]
    fn sectors_map_from_naics_and_campus_land() {
        assert_eq!(sector_of(15, true), Sector::University);
        assert_eq!(sector_of(15, false), Sector::EducationK12);
        assert_eq!(sector_of(16, false), Sector::Healthcare);
        assert_eq!(sector_of(18, true), Sector::RetailFood);
        assert_eq!(sector_of(20, false), Sector::Government);
        assert_eq!(sector_of(12, false), Sector::IndustryTech);
        assert_eq!(sector_of(8, false), Sector::OtherService);
    }

    fn block() -> Block {
        Block { geoid: "261610001001000".into(), tract: "26161000100".into(), lat: 42.28, lon: -83.74, pop: 100,
                housing_units: 50, occupied_units: 40, college_housing: 0, other_group_quarters: 0 }
    }

    fn params() -> PrepareParams {
        serde_json::from_value(json!({
            "state": "26", "county": "161", "place": "03000", "pl_zip": "", "wac_gz": "", "acs_tracts": "", "acs_place": "",
            "osm_pois": "", "osm_buildings": "", "university_operators": [], "hospital_operator": "Michigan Medicine",
            "min_building_m2": 50, "university_address_blocks": [], "campus_polygons": [],
            "assumptions": { "night_share": {"healthcare": 0.25}, "income_low_below": 50000, "income_high_from": 110000,
                "college_housing_undergraduate_fraction": 0.85,
                "poi_weights": {"shopping": {"supermarket": 3}, "medical": {"hospital": 10}} }
        })).unwrap()
    }

    #[test]
    fn a_block_splits_into_owner_and_renter_rows_by_the_tract_renter_share() {
        let tract = Tract { renter_share: Some(0.75), median_income: Some(40000.0), no_vehicle_owner: Some(0.02),
                            no_vehicle_renter: Some(0.2), share_18_24: Some(0.31) };
        let rows = housing_rows(&block(), &tract, 0.5, &params());
        assert_eq!(rows.len(), 2);
        let own = rows.iter().find(|h| h.tenure == Tenure::Own).unwrap();
        let rent = rows.iter().find(|h| h.tenure == Tenure::Rent).unwrap();
        assert_eq!((own.units, rent.units), (10, 30));
        assert_eq!(own.no_vehicle_share, Some(0.02));
        assert_eq!(rent.no_vehicle_share, Some(0.2));
        assert_eq!(rent.building_type, BuildingType::LargeMulti, "30 rental units is an apartment building");
        assert_eq!(rent.median_income_band, Some(IncomeBand::Low));
        assert_eq!(rent.neighborhood, "tract_000100");
        assert_eq!(rent.share_18_24, Some(0.31));
    }

    #[test]
    fn student_housing_and_group_quarters_get_their_own_rows_and_missing_acs_falls_back() {
        let mut b = block();
        b.college_housing = 500;
        b.other_group_quarters = 20;
        let rows = housing_rows(&b, &Tract::default(), 0.5, &params());
        let dorm = rows.iter().find(|h| h.tenure == Tenure::Dorm).unwrap();
        assert_eq!((dorm.units, dorm.building_type, dorm.no_vehicle_share), (500, BuildingType::Dorm, None));
        assert!(rows.iter().any(|h| h.tenure == Tenure::OtherGroup && h.units == 20));
        let rent = rows.iter().find(|h| h.tenure == Tenure::Rent).unwrap();
        assert_eq!(rent.units, 20, "no tract data: the city renter share (0.5) is used");
        assert_eq!(rent.median_income_band, None);
    }

    #[test]
    fn jobs_split_into_day_and_night_rows_and_campus_retail_is_not_university() {
        let mut jobs = [0u32; 20];
        jobs[15] = 100; // CNS16 health care, night share 0.25
        jobs[6] = 40; // CNS07 retail
        let rows = workplace_rows(&block(), &jobs, true, &params().assumptions.night_share);
        let health: Vec<_> = rows.iter().filter(|w| w.sector == Sector::Healthcare).collect();
        assert_eq!(health.len(), 2);
        assert_eq!(health.iter().find(|w| w.shift_pattern == ShiftPattern::Night).unwrap().jobs, 25);
        assert_eq!(health.iter().find(|w| w.shift_pattern == ShiftPattern::StandardDay).unwrap().jobs, 75);
        assert!(health.iter().all(|w| w.is_university), "hospital on university land");
        assert!(rows.iter().filter(|w| w.sector == Sector::RetailFood).all(|w| !w.is_university));
        assert_eq!(rows.iter().map(|w| w.jobs).sum::<u32>(), 140, "no jobs lost or invented");
    }

    #[test]
    fn group_shares_sum_to_one_and_follow_the_jobs() {
        let acs = PlaceAcs { name: "X".into(), adults: 1000.0, seniors: 150.0, undergraduates: 250.0, graduates: 100.0, renter_share: 0.5 };
        let w = |university, shift, jobs| Workplace {
            workplace_id: format!("{university}{jobs}"), lat: 0.0, lon: 0.0, name: None, sector: Sector::OtherService, jobs,
            shift_pattern: shift, is_university: university,
        };
        // working-age adults: 1000 - 250 - 100 - 150 = 500, split 20% / 30% / 50% by jobs
        let jobs = vec![w(true, ShiftPattern::StandardDay, 20), w(false, ShiftPattern::Night, 30), w(false, ShiftPattern::StandardDay, 50)];
        let s = group_shares(&acs, &jobs).unwrap();
        assert!((s.values().sum::<f64>() - 1.0).abs() < 1e-9);
        assert_eq!(s["undergraduate"], 0.25);
        assert_eq!(s["retired"], 0.15);
        assert_eq!(s["university_staff"], 0.1);
        assert_eq!(s["shift_worker"], 0.15);
        assert_eq!(s["resident_worker"], 0.25);
    }

    #[test]
    fn group_shares_refuse_impossible_inputs() {
        let acs = PlaceAcs { name: "X".into(), adults: 100.0, seniors: 50.0, undergraduates: 40.0, graduates: 30.0, renter_share: 0.5 };
        assert!(group_shares(&acs, &[]).is_err());
    }

    #[test]
    fn pois_get_category_weight_and_stay_inside_the_bbox() {
        let p = params();
        let feature = |id: &str, tags: Value, x: f64, y: f64| Feature {
            id: id.into(),
            tags: tags.as_object().unwrap().clone(),
            geom: osm::Geom::Point(x, y),
        };
        let fs = vec![
            feature("n1", json!({"shop": "supermarket", "name": "Kroger"}), -83.7, 42.2),
            feature("n2", json!({"amenity": "hospital"}), -83.7, 42.25),
            feature("n3", json!({"shop": "supermarket"}), -90.0, 42.2),
            feature("n4", json!({"shop": "bakery"}), -83.7, 42.2),
        ];
        let rows = poi_rows(&fs, &p, [-83.85, 42.2, -83.62, 42.34]);
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].poi_id.as_str(), rows[0].category, rows[0].weight), ("mn2", PoiCategory::Medical, 10.0));
        assert_eq!((rows[1].poi_id.as_str(), rows[1].category, rows[1].weight), ("sn1", PoiCategory::Shopping, 3.0));
    }
}
