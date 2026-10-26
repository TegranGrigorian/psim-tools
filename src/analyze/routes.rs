//! `routes.geojson`: transit route lines for a scenario, from the raw GTFS feeds.
//!
//! For each route we draw the shape used by most of its trips on the service
//! date, and count those trips (`trips_per_weekday`).

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};
use chrono::{Datelike, NaiveDate};
use serde_json::{Value, json};
use zip::ZipArchive;

use crate::config::CityConfig;

pub type RouteId = (String, String); // (agency folder, GTFS route_id)

/// Keep a shape point only if it is at least this far (degrees) from the last kept one.
const MIN_STEP_DEGREES: f64 = 0.00015; // about 15 m

fn table<R: Read>(reader: R) -> Result<(Vec<String>, Vec<csv::StringRecord>)> {
    let mut rdr = csv::ReaderBuilder::new().flexible(true).from_reader(reader);
    let headers: Vec<String> = rdr
        .headers()?
        .iter()
        .map(|h| h.trim_start_matches('\u{feff}').trim().to_string())
        .collect();
    let rows = rdr.records().collect::<Result<Vec<_>, _>>()?;
    Ok((headers, rows))
}

fn col(headers: &[String], name: &str) -> Result<usize> {
    headers.iter().position(|h| h == name).with_context(|| format!("GTFS column `{name}` missing"))
}

fn read_table(archive: &mut ZipArchive<File>, name: &str) -> Result<Option<(Vec<String>, Vec<csv::StringRecord>)>> {
    match archive.by_name(name) {
        Ok(entry) => Ok(Some(table(entry)?)),
        Err(zip::result::ZipError::FileNotFound) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Service ids running on `date` (calendar.txt plus calendar_dates.txt exceptions).
fn active_services(archive: &mut ZipArchive<File>, date: NaiveDate) -> Result<BTreeSet<String>> {
    let ymd = date.format("%Y%m%d").to_string();
    let weekday = ["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"]
        [date.weekday().num_days_from_monday() as usize];
    let mut active = BTreeSet::new();
    if let Some((h, rows)) = read_table(archive, "calendar.txt")? {
        let (sid, start, end, day) =
            (col(&h, "service_id")?, col(&h, "start_date")?, col(&h, "end_date")?, col(&h, weekday)?);
        for r in rows {
            if r[start] <= *ymd && *ymd <= r[end] && &r[day] == "1" {
                active.insert(r[sid].to_string());
            }
        }
    }
    if let Some((h, rows)) = read_table(archive, "calendar_dates.txt")? {
        let (sid, d, kind) = (col(&h, "service_id")?, col(&h, "date")?, col(&h, "exception_type")?);
        for r in rows.iter().filter(|r| r[d] == *ymd) {
            if &r[kind] == "1" {
                active.insert(r[sid].to_string());
            } else {
                active.remove(&r[sid]);
            }
        }
    }
    Ok(active)
}

fn feed_routes(zip_path: &Path, agency: &str, date: NaiveDate) -> Result<Vec<(RouteId, Value)>> {
    let mut archive = ZipArchive::new(File::open(zip_path)?)
        .with_context(|| format!("opening {}", zip_path.display()))?;
    let active = active_services(&mut archive, date)?;

    let (h, rows) = read_table(&mut archive, "routes.txt")?.context("routes.txt missing")?;
    let (rid, short, long) = (col(&h, "route_id")?, col(&h, "route_short_name")?, col(&h, "route_long_name").unwrap_or(usize::MAX));
    let names: BTreeMap<String, String> = rows
        .iter()
        .map(|r| {
            let short_name = r[short].trim();
            let name = if short_name.is_empty() { r.get(long).unwrap_or("").trim() } else { short_name };
            (r[rid].to_string(), name.to_string())
        })
        .collect();

    // trips per (route, shape) on the service date
    let (h, rows) = read_table(&mut archive, "trips.txt")?.context("trips.txt missing")?;
    let (t_route, t_service, t_shape) = (col(&h, "route_id")?, col(&h, "service_id")?, col(&h, "shape_id")?);
    let mut per_route: BTreeMap<String, usize> = BTreeMap::new();
    let mut per_shape: BTreeMap<(String, String), usize> = BTreeMap::new();
    for r in rows.iter().filter(|r| active.contains(&r[t_service])) {
        *per_route.entry(r[t_route].to_string()).or_default() += 1;
        if !r[t_shape].is_empty() {
            *per_shape.entry((r[t_route].to_string(), r[t_shape].to_string())).or_default() += 1;
        }
    }
    // most-used shape per route; ties go to the smaller shape id
    let mut best: BTreeMap<String, (usize, String)> = BTreeMap::new();
    for ((route, shape), n) in per_shape {
        let entry = best.entry(route).or_insert((0, String::new()));
        if n > entry.0 {
            *entry = (n, shape);
        }
    }
    let wanted: BTreeSet<&String> = best.values().map(|(_, s)| s).collect();

    let mut points: BTreeMap<String, Vec<(u64, f64, f64)>> = BTreeMap::new();
    if let Some((h, rows)) = read_table(&mut archive, "shapes.txt")? {
        let (sid, lat, lon, seq) =
            (col(&h, "shape_id")?, col(&h, "shape_pt_lat")?, col(&h, "shape_pt_lon")?, col(&h, "shape_pt_sequence")?);
        for r in rows.iter().filter(|r| wanted.contains(&r[sid].to_string())) {
            points.entry(r[sid].to_string()).or_default().push((
                r[seq].trim().parse().unwrap_or(0),
                r[lat].trim().parse()?,
                r[lon].trim().parse()?,
            ));
        }
    }

    let mut out = Vec::new();
    for (route, (_, shape)) in best {
        let Some(mut pts) = points.remove(&shape) else { continue };
        pts.sort_by_key(|p| p.0);
        let mut line: Vec<[f64; 2]> = Vec::new();
        for (i, &(_, lat, lon)) in pts.iter().enumerate() {
            let far = line.last().is_none_or(|l| (l[0] - lon).abs().max((l[1] - lat).abs()) >= MIN_STEP_DEGREES);
            if far || i == pts.len() - 1 {
                line.push([(lon * 1e5).round() / 1e5, (lat * 1e5).round() / 1e5]);
            }
        }
        if line.len() < 2 {
            continue;
        }
        let name = names.get(&route).cloned().unwrap_or_default();
        out.push((
            (agency.to_string(), route.clone()),
            json!({
                "type": "Feature",
                "properties": {
                    "route_id": route,
                    "route_short_name": name,
                    "agency": agency,
                    "trips_per_weekday": per_route.get(&route).copied().unwrap_or(0),
                },
                "geometry": { "type": "LineString", "coordinates": line },
            }),
        ));
    }
    Ok(out)
}

/// Route lines for every feed of `scenario`. `baseline` marks which routes are not new.
pub fn routes_geojson(
    cfg: &CityConfig,
    city_dir: &Path,
    scenario: &str,
    baseline: Option<&BTreeSet<RouteId>>,
) -> Result<(Value, BTreeSet<RouteId>)> {
    let folders = cfg.gtfs.get(scenario).with_context(|| format!("scenario `{scenario}` not in config"))?;
    let date = NaiveDate::parse_from_str(&cfg.service_date, "%Y-%m-%d")?;

    let mut features = Vec::new();
    let mut ids = BTreeSet::new();
    for folder in folders {
        let dir = city_dir.join(folder);
        let agency = dir.file_name().and_then(|n| n.to_str()).unwrap_or("feed").to_string();
        let zips: Vec<_> = std::fs::read_dir(&dir)
            .with_context(|| format!("reading {} (run data/<city>/fetch.sh)", dir.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "zip"))
            .collect();
        if zips.is_empty() {
            bail!("no GTFS zip in {}", dir.display());
        }
        for zip_path in zips {
            for (id, mut feature) in feed_routes(&zip_path, &agency, date)? {
                if let Some(base) = baseline {
                    feature["properties"]["is_new"] = json!(!base.contains(&id));
                }
                ids.insert(id);
                features.push(feature);
            }
        }
    }
    let collection = json!({
        "type": "FeatureCollection",
        "schema_version": crate::schema::SCHEMA_VERSION,
        "fake": cfg.sample_data,
        "city_slug": cfg.city_slug,
        "scenario": scenario,
        "features": features,
    });
    Ok((collection, ids))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tiny_feed(dir: &Path) {
        let mut z = zip::ZipWriter::new(File::create(dir.join("google_transit.zip")).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        let mut put = |name: &str, text: &str| {
            z.start_file(name, opts).unwrap();
            z.write_all(text.as_bytes()).unwrap();
        };
        put("routes.txt", "route_id,route_short_name,route_long_name\nR1,1,One\nR2,,Two Long\nR3,3,Never runs\n");
        put("calendar.txt", "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\nWK,1,1,1,1,1,0,0,20260901,20261231\nSAT,0,0,0,0,0,1,0,20260901,20261231\n");
        put("calendar_dates.txt", "service_id,date,exception_type\nWK,20261110,1\n");
        put("trips.txt", "route_id,service_id,trip_id,shape_id\nR1,WK,a,SH1\nR1,WK,b,SH1\nR1,WK,c,SH2\nR2,WK,d,SH3\nR3,SAT,e,SH4\n");
        put("shapes.txt", "\u{feff}shape_id,shape_pt_lat,shape_pt_lon,shape_pt_sequence\nSH1,42.280,-83.740,2\nSH1,42.270,-83.750,1\nSH1,42.290,-83.730,3\nSH2,42.0,-83.0,1\nSH2,42.1,-83.1,2\nSH3,42.30,-83.70,1\nSH3,42.30000001,-83.70000001,2\nSH3,42.31,-83.71,3\nSH4,42.0,-83.0,1\nSH4,42.1,-83.1,2\n");
        z.finish().unwrap();
    }

    #[test]
    fn routes_use_the_busiest_shape_active_on_the_service_date() {
        let dir = std::env::temp_dir().join(format!("psim_routes_{}", std::process::id()));
        let feed = dir.join("raw/gtfs/agency");
        std::fs::create_dir_all(&feed).unwrap();
        tiny_feed(&feed);
        let routes = feed_routes(&feed.join("google_transit.zip"), "agency", NaiveDate::from_ymd_opt(2026, 11, 10).unwrap()).unwrap();
        let by_id: BTreeMap<String, &Value> = routes.iter().map(|((_, id), v)| (id.clone(), v)).collect();

        assert!(!by_id.contains_key("R3"), "Saturday-only route doesn't run on a Tuesday");
        let r1 = by_id["R1"];
        assert_eq!(r1["properties"]["trips_per_weekday"], 3);
        let line = r1["geometry"]["coordinates"].as_array().unwrap();
        assert_eq!(line.len(), 3, "SH1 (2 trips) beats SH2 (1 trip)");
        assert_eq!(line[0], json!([-83.75, 42.27]), "points are ordered by sequence, as [lon, lat]");
        assert_eq!(by_id["R2"]["properties"]["route_short_name"], "Two Long", "falls back to the long name");
        assert_eq!(by_id["R2"]["geometry"]["coordinates"].as_array().unwrap().len(), 2, "near-duplicate point thinned");
        let _ = std::fs::remove_dir_all(dir);
    }
}
