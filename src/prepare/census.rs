//! Readers for the Census files: the 2020 redistricting file (blocks), LODES
//! workplace jobs, and the ACS tables downloaded by `fetch_census.py`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use serde_json::Value;
use zip::ZipArchive;

// Field positions (0-based) in the 2020 PL 94-171 files; checked against the real files.
// Geographic header (migeo2020.pl):
const GEO_SUMLEV: usize = 2;
const GEO_LOGRECNO: usize = 7;
const GEO_BLOCK_GEOID: usize = 9; // 15 digits: state, county, tract, block
const GEO_STATE: usize = 12;
const GEO_COUNTY: usize = 14;
const GEO_PLACE: usize = 29;
const GEO_TRACT: usize = 32;
const GEO_POP100: usize = 90;
const GEO_LAT: usize = 92;
const GEO_LON: usize = 93;
// File 02 (H1 housing units): the last three fields are total, occupied, vacant.
// File 03 (P5 group quarters), after 5 leading fields:
const P5_NURSING: usize = 9;
const P5_OTHER_INSTITUTIONAL: usize = 10;
const P5_COLLEGE: usize = 12;
const P5_OTHER_NONINSTITUTIONAL: usize = 14;
const LOGRECNO: usize = 4;

#[derive(Debug, Clone)]
pub struct Block {
    pub geoid: String,
    /// 11-digit tract GEOID
    pub tract: String,
    pub lat: f64,
    pub lon: f64,
    pub pop: u32,
    pub housing_units: u32,
    pub occupied_units: u32,
    /// Population in college/university student housing
    pub college_housing: u32,
    /// Nursing homes, other institutional and other non-institutional group quarters
    pub other_group_quarters: u32,
}

fn lines(zip: &mut ZipArchive<File>, name: &str) -> Result<impl Iterator<Item = Result<Vec<String>>>> {
    let entry = zip.by_name(name).with_context(|| format!("{name} not in the PL zip"))?;
    let mut reader = BufReader::new(entry);
    let mut buf = Vec::new();
    // The files are large and may not be UTF-8: read bytes, convert lossily.
    let mut rows: Vec<Result<Vec<String>>> = Vec::new();
    loop {
        buf.clear();
        if reader.read_until(b'\n', &mut buf)? == 0 {
            break;
        }
        let line = String::from_utf8_lossy(&buf);
        rows.push(Ok(line.trim_end().split('|').map(str::to_string).collect()));
    }
    Ok(rows.into_iter())
}

fn num(s: &str) -> u32 {
    s.trim().parse().unwrap_or(0)
}

/// Blocks of one place (e.g. Ann Arbor city) with housing and group quarters.
pub fn read_blocks(pl_zip: &Path, state: &str, county: &str, place: &str) -> Result<Vec<Block>> {
    let mut zip = ZipArchive::new(File::open(pl_zip).with_context(|| format!("opening {}", pl_zip.display()))?)?;

    let mut blocks: Vec<Block> = Vec::new();
    let mut by_record: HashMap<String, usize> = HashMap::new();
    for row in lines(&mut zip, "migeo2020.pl")? {
        let f = row?;
        if f.len() <= GEO_LON || f[GEO_SUMLEV] != "750" || f[GEO_STATE] != state || f[GEO_COUNTY] != county || f[GEO_PLACE] != place {
            continue;
        }
        by_record.insert(f[GEO_LOGRECNO].clone(), blocks.len());
        blocks.push(Block {
            geoid: f[GEO_BLOCK_GEOID].clone(),
            tract: format!("{state}{county}{}", f[GEO_TRACT]),
            lat: f[GEO_LAT].trim().parse().context("block latitude")?,
            lon: f[GEO_LON].trim().parse().context("block longitude")?,
            pop: num(&f[GEO_POP100]),
            housing_units: 0,
            occupied_units: 0,
            college_housing: 0,
            other_group_quarters: 0,
        });
    }
    if blocks.is_empty() {
        bail!("no blocks for state {state} county {county} place {place} in {}", pl_zip.display());
    }

    for row in lines(&mut zip, "mi000022020.pl")? {
        let f = row?;
        if let Some(&i) = f.get(LOGRECNO).and_then(|r| by_record.get(r)) {
            let n = f.len();
            blocks[i].housing_units = num(&f[n - 3]);
            blocks[i].occupied_units = num(&f[n - 2]);
        }
    }
    for row in lines(&mut zip, "mi000032020.pl")? {
        let f = row?;
        if let Some(&i) = f.get(LOGRECNO).and_then(|r| by_record.get(r)) {
            blocks[i].college_housing = num(&f[P5_COLLEGE]);
            blocks[i].other_group_quarters =
                num(&f[P5_NURSING]) + num(&f[P5_OTHER_INSTITUTIONAL]) + num(&f[P5_OTHER_NONINSTITUTIONAL]);
        }
    }
    Ok(blocks)
}

/// Jobs by block of workplace: the 20 NAICS-sector columns `CNS01`..`CNS20`.
pub fn read_wac(path: &Path, geoids: &HashSet<&str>) -> Result<BTreeMap<String, [u32; 20]>> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut rdr = csv::Reader::from_reader(GzDecoder::new(file));
    let headers = rdr.headers()?.clone();
    let geo = headers.iter().position(|h| h == "w_geocode").context("w_geocode column")?;
    let cols: Vec<usize> = (1..=20)
        .map(|i| headers.iter().position(|h| h == format!("CNS{i:02}")).with_context(|| format!("CNS{i:02} column")))
        .collect::<Result<_>>()?;
    let mut out = BTreeMap::new();
    for rec in rdr.records() {
        let rec = rec?;
        if geoids.contains(&rec[geo]) {
            let mut jobs = [0u32; 20];
            for (k, &c) in cols.iter().enumerate() {
                jobs[k] = rec[c].parse().unwrap_or(0);
            }
            out.insert(rec[geo].to_string(), jobs);
        }
    }
    Ok(out)
}

/// ACS values for one tract. `None` where the Census suppressed or has no estimate.
#[derive(Debug, Clone, Default)]
pub struct Tract {
    pub renter_share: Option<f64>,
    pub median_income: Option<f64>,
    pub no_vehicle_owner: Option<f64>,
    pub no_vehicle_renter: Option<f64>,
    pub share_18_24: Option<f64>,
}

/// A number from the API's string cells; negative sentinels (-666666666) are "no estimate".
fn cell(row: &Value, key: &str) -> Option<f64> {
    row.get(key)?.as_str()?.trim().parse::<f64>().ok().filter(|v| *v >= 0.0)
}

fn ratio(n: Option<f64>, d: Option<f64>) -> Option<f64> {
    match (n, d) {
        (Some(n), Some(d)) if d > 0.0 => Some(n / d),
        _ => None,
    }
}

pub fn read_tracts(path: &Path) -> Result<HashMap<String, Tract>> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?)?;
    let mut out = HashMap::new();
    for row in v["tracts"].as_array().context("tracts array")? {
        let geoid = format!(
            "{}{}{}",
            row["state"].as_str().unwrap_or(""),
            row["county"].as_str().unwrap_or(""),
            row["tract"].as_str().unwrap_or("")
        );
        let young: Option<f64> = ["B01001_007E", "B01001_008E", "B01001_009E", "B01001_010E", "B01001_031E", "B01001_032E", "B01001_033E", "B01001_034E"]
            .iter()
            .map(|k| cell(row, k))
            .sum::<Option<f64>>();
        out.insert(
            geoid,
            Tract {
                renter_share: ratio(cell(row, "B25003_003E"), cell(row, "B25003_001E")),
                median_income: cell(row, "B19013_001E"),
                no_vehicle_owner: ratio(cell(row, "B25044_003E"), cell(row, "B25044_002E")),
                no_vehicle_renter: ratio(cell(row, "B25044_010E"), cell(row, "B25044_009E")),
                share_18_24: ratio(young, cell(row, "B01001_001E")),
            },
        );
    }
    Ok(out)
}

/// ACS totals for the whole city, used for the group shares.
#[derive(Debug, Clone)]
pub struct PlaceAcs {
    pub name: String,
    pub adults: f64,
    pub seniors: f64,
    pub undergraduates: f64,
    pub graduates: f64,
    pub renter_share: f64,
}

pub fn read_place(path: &Path) -> Result<PlaceAcs> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?)?;
    let vals = &v["values"];
    let get = |k: &str| cell(vals, k).with_context(|| format!("ACS place value {k}"));
    Ok(PlaceAcs {
        name: v["name"].as_str().unwrap_or("").to_string(),
        adults: v["adults_18_plus"].as_f64().context("adults_18_plus")?,
        seniors: v["adults_65_plus"].as_f64().context("adults_65_plus")?,
        undergraduates: get("B14001_008E")?,
        graduates: get("B14001_009E")?,
        renter_share: get("B25003_003E")? / get("B25003_001E")?,
    })
}
