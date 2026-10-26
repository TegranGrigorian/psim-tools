//! Builds the summary files the site reads (SCHEMA section 5) from replicates.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::aggregate::{DIMENSIONS, Key, METRICS, Replicate};
use super::geo;
use super::stats::{Interval, interval, mean, round};
use crate::config::CityConfig;
use crate::schema::{Group, Housing, SCHEMA_VERSION};

fn iv(i: Interval) -> Value {
    json!({ "mean": round(i.mean, 4), "ci_low": round(i.ci_low, 4), "ci_high": round(i.ci_high, 4) })
}

/// Every (dimension, value) seen in any replicate, in a stable, readable order.
fn keys(reps: &[Replicate]) -> Vec<Key> {
    let all: BTreeSet<Key> = reps.iter().flat_map(|r| r.groups.keys().cloned()).collect();
    let mut out: Vec<Key> = Vec::new();
    for dim in DIMENSIONS {
        let mut values: Vec<&Key> = all.iter().filter(|(d, _)| d == dim).collect();
        if dim == "group" {
            values.sort_by_key(|(_, v)| Group::ALL.iter().position(|g| g.name() == v).unwrap_or(usize::MAX));
        }
        out.extend(values.into_iter().cloned());
    }
    out
}

fn values_of(reps: &[Replicate], key: &Key, metric: &str) -> Vec<f64> {
    reps.iter().filter_map(|r| r.groups.get(key)).filter_map(|m| m.get(metric)).collect()
}

fn n_personas_mean(reps: &[Replicate], key: &Key) -> f64 {
    let total: f64 = reps.iter().map(|r| r.groups.get(key).map_or(0.0, |m| m.n_personas)).sum();
    total / reps.len() as f64
}

/// The six metrics as intervals, or None if any has no data in any replicate.
fn metric_block(reps: &[Replicate], key: &Key) -> Option<serde_json::Map<String, Value>> {
    let mut block = serde_json::Map::new();
    for m in METRICS {
        block.insert(m.to_string(), iv(interval(&values_of(reps, key, m))?));
    }
    // Not in the site's MetricKey list; null when the group has no data for them (for example
    // no work or class trips, or none went through the delay model).
    for extra in ["required_failure_rate", "on_time_rate", "delay_buffer_minutes", "planned_minutes", "allow_minutes"] {
        block.insert(extra.to_string(), interval(&values_of(reps, key, extra)).map_or(Value::Null, iv));
    }
    Some(block)
}

/// Returns the file's JSON and the entries left out for lack of data.
pub fn group_metrics(cfg: &CityConfig, scenario: &str, reps: &[Replicate]) -> (Value, Vec<Key>) {
    let mut groups = Vec::new();
    let mut skipped = Vec::new();
    for key in keys(reps) {
        match metric_block(reps, &key) {
            Some(metrics) => groups.push(json!({
                "dimension": key.0,
                "value": key.1,
                "n_personas_mean": round(n_personas_mean(reps, &key), 1),
                "metrics": metrics,
            })),
            None => skipped.push(key),
        }
    }
    let value = json!({
        "schema_version": SCHEMA_VERSION,
        "fake": cfg.sample_data,
        "city_slug": cfg.city_slug,
        "scenario": scenario,
        "n_replicates": reps.len(),
        "interval": "empirical 2.5th to 97.5th percentile across replicates",
        "groups": groups,
    });
    (value, skipped)
}

pub fn time_of_day(cfg: &CityConfig, scenario: &str, reps: &[Replicate]) -> Value {
    let mut series = Vec::new();
    for group in Group::ALL {
        for day_type in ["weekday", "weekend"] {
            // pooled over replicates
            let mut hours = Vec::new();
            let mut any = false;
            for hour in 0..24u32 {
                let (mut trips, mut failed, mut wait_sum, mut wait_n) = (0u64, 0u64, 0.0, 0u64);
                for r in reps {
                    if let Some(h) = r.hours.get(&(group.name().to_string(), day_type, hour)) {
                        trips += h.trips;
                        failed += h.failed;
                        wait_sum += h.wait_sum;
                        wait_n += h.wait_n;
                    }
                }
                any |= trips > 0;
                // Hours with no trips carry zeros and `n_trips: 0`; the site must not read them as data.
                hours.push(json!({
                    "hour": hour,
                    "failure_rate": round(if trips > 0 { failed as f64 / trips as f64 } else { 0.0 }, 4),
                    "mean_wait": round(if wait_n > 0 { wait_sum / wait_n as f64 } else { 0.0 }, 2),
                    "n_trips": trips,
                    "n_wait_samples": wait_n,
                }));
            }
            if any {
                series.push(json!({ "group": group.name(), "day_type": day_type, "hours": hours }));
            }
        }
    }
    json!({
        "schema_version": SCHEMA_VERSION,
        "fake": cfg.sample_data,
        "city_slug": cfg.city_slug,
        "scenario": scenario,
        "day_types": ["weekday", "weekend"],
        "note": "weekday = mon-fri, weekend = sat; hour is the desired departure hour; pooled over replicates",
        "series": series,
    })
}

fn title_case(slug: &str) -> String {
    slug.split('_')
        .map(|w| {
            let mut c = w.chars();
            c.next().map_or(String::new(), |f| f.to_uppercase().collect::<String>() + c.as_str())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn neighborhood_map(cfg: &CityConfig, scenario: &str, reps: &[Replicate], housing: &[Housing]) -> Value {
    let mut features = Vec::new();
    for key in keys(reps).into_iter().filter(|(d, _)| d == "neighborhood") {
        let points: Vec<(f64, f64)> =
            housing.iter().filter(|h| h.neighborhood == key.1).map(|h| (h.lon, h.lat)).collect();
        let (Some(metrics), false) = (metric_block(reps, &key), points.is_empty()) else { continue };
        features.push(json!({
            "type": "Feature",
            "properties": {
                "neighborhood": key.1,
                "name": title_case(&key.1),
                "n_personas_mean": round(n_personas_mean(reps, &key), 1),
                "metrics": metrics,
            },
            "geometry": { "type": "Polygon", "coordinates": [geo::outline(&points)] },
        }));
    }
    json!({
        "type": "FeatureCollection",
        "schema_version": SCHEMA_VERSION,
        "fake": cfg.sample_data,
        "city_slug": cfg.city_slug,
        "scenario": scenario,
        "outline_note": "approximate outlines (padded convex hull of housing points), not official boundaries",
        "features": features,
    })
}

/// Paired by seed: replicate i of `before` and `after` share a persona sample,
/// so each delta isolates the change in service.
pub fn comparison(cfg: &CityConfig, from: &str, to: &str, before: &[Replicate], after: &[Replicate]) -> Value {
    let after_by_seed: BTreeMap<u64, &Replicate> = after.iter().map(|r| (r.seed, r)).collect();
    let pairs: Vec<(&Replicate, &Replicate)> =
        before.iter().filter_map(|b| after_by_seed.get(&b.seed).map(|a| (b, *a))).collect();

    let mut groups = Vec::new();
    for key in keys(before) {
        let mut deltas = serde_json::Map::new();
        for m in METRICS {
            let both: Vec<(f64, f64)> = pairs
                .iter()
                .filter_map(|(b, a)| Some((b.groups.get(&key)?.get(m)?, a.groups.get(&key)?.get(m)?)))
                .collect();
            let diffs: Vec<f64> = both.iter().map(|(b, a)| a - b).collect();
            let (Some(i), Some(b_mean), Some(a_mean)) = (
                interval(&diffs),
                mean(&both.iter().map(|p| p.0).collect::<Vec<_>>()),
                mean(&both.iter().map(|p| p.1).collect::<Vec<_>>()),
            ) else {
                break;
            };
            deltas.insert(
                m.to_string(),
                json!({
                    "before": round(b_mean, 4), "after": round(a_mean, 4), "delta": round(i.mean, 4),
                    "ci_low": round(i.ci_low, 4), "ci_high": round(i.ci_high, 4),
                }),
            );
        }
        if deltas.len() == METRICS.len() {
            groups.push(json!({ "dimension": key.0, "value": key.1, "deltas": deltas }));
        }
    }
    json!({
        "schema_version": SCHEMA_VERSION,
        "fake": cfg.sample_data,
        "city_slug": cfg.city_slug,
        "from_scenario": from,
        "to_scenario": to,
        "n_paired_replicates": pairs.len(),
        "interval": "empirical 2.5th to 97.5th percentile of the per-replicate differences (after - before)",
        "groups": groups,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze::aggregate::{HourAcc, RepMetrics};
    use std::collections::BTreeMap;

    fn cfg(sample: bool) -> CityConfig {
        serde_json::from_value(json!({
            "city_slug": "x", "display_name": "X", "timezone": "America/Detroit", "sample_data": sample,
            "bbox": [-83.85, 42.2, -83.6, 42.3], "population_size": 10, "represented_population": 100,
            "n_replicates": 3, "base_seed": null, "gtfs": {"baseline": ["a"]}, "osm": "o",
            "service_date": "2026-11-10", "weekend_service_date": "2026-11-14",
            "departure_window_minutes": 30, "max_walk_minutes": 20, "max_trip_minutes": 120,
            "group_shares": {"retired": 1.0}
        }))
        .unwrap()
    }

    fn metrics(total: f64, fail: f64) -> RepMetrics {
        RepMetrics {
            n_personas: 10.0,
            total_minutes: Some(total),
            wait_minutes: Some(5.0),
            in_vehicle_minutes: Some(10.0),
            walk_minutes: Some(8.0),
            transfers: Some(0.5),
            failure_rate: Some(fail),
            required_failure_rate: None,
            on_time_rate: None,
            delay_buffer_minutes: None,
            planned_minutes: None,
        }
    }

    fn rep(seed: u64, total: f64) -> Replicate {
        let mut groups = BTreeMap::new();
        groups.insert(("group".to_string(), "retired".to_string()), metrics(total, 0.1));
        Replicate { seed, groups, hours: BTreeMap::new() }
    }

    #[test]
    fn group_metrics_averages_replicates_and_flags_sample_data() {
        let reps: Vec<_> = (0..4).map(|i| rep(i, 10.0 * (i + 1) as f64)).collect();
        let (v, skipped) = group_metrics(&cfg(true), "baseline", &reps);
        assert!(skipped.is_empty());
        assert_eq!(v["fake"], true);
        assert_eq!(v["n_replicates"], 4);
        let m = &v["groups"][0]["metrics"]["total_minutes"];
        assert_eq!(m["mean"], 25.0);
        assert!(m["ci_low"].as_f64().unwrap() < 12.0 && m["ci_high"].as_f64().unwrap() > 38.0);
        assert!(v["groups"][0]["metrics"]["required_failure_rate"].is_null());
        assert_eq!(group_metrics(&cfg(false), "baseline", &reps).0["fake"], false);
    }

    #[test]
    fn a_group_with_a_metric_nobody_measured_is_skipped_not_zeroed() {
        let mut r = rep(1, 10.0);
        r.groups.get_mut(&("group".into(), "retired".into())).unwrap().wait_minutes = None;
        let (v, skipped) = group_metrics(&cfg(false), "baseline", &[r]);
        assert_eq!(v["groups"].as_array().unwrap().len(), 0);
        assert_eq!(skipped.len(), 1);
    }

    #[test]
    fn comparison_pairs_replicates_by_seed_not_by_position() {
        let before = vec![rep(1, 30.0), rep(2, 40.0), rep(3, 50.0)];
        // same seeds, listed in another order, each 5 minutes faster; seed 9 has no partner
        let after = vec![rep(3, 45.0), rep(9, 1.0), rep(1, 25.0), rep(2, 35.0)];
        let v = comparison(&cfg(false), "baseline", "redesign_v1", &before, &after);
        assert_eq!(v["n_paired_replicates"], 3);
        let d = &v["groups"][0]["deltas"]["total_minutes"];
        assert_eq!(d["before"], 40.0);
        assert_eq!(d["after"], 35.0);
        assert_eq!(d["delta"], -5.0);
        assert_eq!(d["ci_low"], -5.0);
        assert_eq!(d["ci_high"], -5.0);
    }

    #[test]
    fn time_of_day_pools_replicates_and_marks_empty_hours() {
        let mut a = rep(1, 10.0);
        a.hours.insert(("retired".into(), "weekday", 9), HourAcc { trips: 10, failed: 1, wait_sum: 20.0, wait_n: 4 });
        let mut b = rep(2, 10.0);
        b.hours.insert(("retired".into(), "weekday", 9), HourAcc { trips: 30, failed: 3, wait_sum: 0.0, wait_n: 0 });
        let v = time_of_day(&cfg(false), "baseline", &[a, b]);
        let series = v["series"].as_array().unwrap();
        assert_eq!(series.len(), 1, "only groups and day types with trips");
        let hours = series[0]["hours"].as_array().unwrap();
        assert_eq!(hours.len(), 24);
        assert_eq!(hours[9]["failure_rate"], 0.1);
        assert_eq!(hours[9]["mean_wait"], 5.0);
        assert_eq!(hours[9]["n_trips"], 40);
        assert_eq!(hours[3]["n_trips"], 0);
    }

    #[test]
    fn neighborhood_names_are_readable() {
        assert_eq!(title_case("old_west_side"), "Old West Side");
        assert_eq!(title_case("northside"), "Northside");
    }
}
