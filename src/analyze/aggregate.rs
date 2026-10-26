//! One replicate's results, reduced to the numbers the summaries are built from.
//!
//! Metrics are per (dimension, value), e.g. ("group", "undergraduate"). A trip
//! counts toward each dimension value its persona has. Definitions:
//! - `total_minutes`: mean over completed trips (all of them).
//! - `wait/in_vehicle/walk_minutes`, `transfers`: mean over completed trips in the
//!   engine's detailed sample (`breakdown_sampled`); `None` if the sample has none.
//! - `failure_rate`: share of trips that did not complete.
//! - `required_failure_rate`: the same for work and class trips only.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use serde::Serialize;

use super::results::TripResult;
use crate::schema::{Day, Persona, Trip};

pub const DIMENSIONS: [&str; 6] =
    ["group", "tenure", "neighborhood", "shift_pattern", "age_band", "income_band"];

/// The metrics every group entry carries (`MetricKey` in the site's types).
pub const METRICS: [&str; 6] = [
    "total_minutes",
    "wait_minutes",
    "in_vehicle_minutes",
    "walk_minutes",
    "transfers",
    "failure_rate",
];

pub type Key = (String, String); // (dimension, value)

#[derive(Debug, Clone, Default)]
pub struct RepMetrics {
    pub n_personas: f64,
    pub total_minutes: Option<f64>,
    pub wait_minutes: Option<f64>,
    pub in_vehicle_minutes: Option<f64>,
    pub walk_minutes: Option<f64>,
    pub transfers: Option<f64>,
    pub failure_rate: Option<f64>,
    pub required_failure_rate: Option<f64>,
}

impl RepMetrics {
    pub fn get(&self, metric: &str) -> Option<f64> {
        match metric {
            "total_minutes" => self.total_minutes,
            "wait_minutes" => self.wait_minutes,
            "in_vehicle_minutes" => self.in_vehicle_minutes,
            "walk_minutes" => self.walk_minutes,
            "transfers" => self.transfers,
            "failure_rate" => self.failure_rate,
            "required_failure_rate" => self.required_failure_rate,
            _ => None,
        }
    }
}

/// Failures and waits for one (group, day type, hour of departure).
#[derive(Debug, Clone, Default)]
pub struct HourAcc {
    pub trips: u64,
    pub failed: u64,
    pub wait_sum: f64,
    pub wait_n: u64,
}

#[derive(Debug, Clone)]
pub struct Replicate {
    pub seed: u64,
    pub groups: BTreeMap<Key, RepMetrics>,
    /// (group, "weekday" | "weekend", hour 0..=23)
    pub hours: BTreeMap<(String, &'static str, u32), HourAcc>,
}

#[derive(Default)]
struct Acc {
    personas: u64,
    trips: u64,
    completed: u64,
    total_sum: f64,
    detail_n: u64,
    wait_sum: f64,
    in_vehicle_sum: f64,
    walk_sum: f64,
    transfers_sum: f64,
    required: u64,
    required_failed: u64,
}

/// A schema enum's file spelling, e.g. `Group::UniversityStaff` -> "university_staff".
pub fn enum_name<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(s)) => s,
        other => format!("{other:?}"),
    }
}

fn dimensions(p: &Persona) -> [(&'static str, String); 6] {
    [
        ("group", enum_name(&p.group)),
        ("tenure", enum_name(&p.tenure)),
        ("neighborhood", p.neighborhood.clone()),
        ("shift_pattern", enum_name(&p.shift_pattern)),
        ("age_band", enum_name(&p.age_band)),
        ("income_band", enum_name(&p.income_band)),
    ]
}

fn day_type(day: Day) -> &'static str {
    match day {
        Day::Sat | Day::Sun => "weekend",
        _ => "weekday",
    }
}

fn departure_hour(depart_time: &str) -> Result<u32> {
    let hour: u32 = depart_time
        .split(':')
        .next()
        .and_then(|h| h.parse().ok())
        .ok_or_else(|| anyhow::anyhow!("bad depart_time `{depart_time}`"))?;
    if hour > 23 {
        bail!("bad depart_time `{depart_time}`");
    }
    Ok(hour)
}

fn finish(a: &Acc) -> RepMetrics {
    let mean = |sum: f64, n: u64| (n > 0).then(|| sum / n as f64);
    let rate = |failed: u64, n: u64| (n > 0).then(|| failed as f64 / n as f64);
    RepMetrics {
        n_personas: a.personas as f64,
        total_minutes: mean(a.total_sum, a.completed),
        wait_minutes: mean(a.wait_sum, a.detail_n),
        in_vehicle_minutes: mean(a.in_vehicle_sum, a.detail_n),
        walk_minutes: mean(a.walk_sum, a.detail_n),
        transfers: mean(a.transfers_sum, a.detail_n),
        failure_rate: rate(a.trips - a.completed, a.trips),
        required_failure_rate: rate(a.required_failed, a.required),
    }
}

pub fn aggregate(
    seed: u64,
    results: &[TripResult],
    personas: &[Persona],
    trips: &[Trip],
) -> Result<Replicate> {
    let persona_by_id: BTreeMap<&str, &Persona> =
        personas.iter().map(|p| (p.persona_id.as_str(), p)).collect();
    let trip_by_id: BTreeMap<&str, &Trip> = trips.iter().map(|t| (t.trip_id.as_str(), t)).collect();

    let mut accs: BTreeMap<Key, Acc> = BTreeMap::new();
    for p in personas {
        for (dim, value) in dimensions(p) {
            accs.entry((dim.to_string(), value)).or_default().personas += 1;
        }
    }

    let mut hours: BTreeMap<(String, &'static str, u32), HourAcc> = BTreeMap::new();
    for r in results {
        let Some(trip) = trip_by_id.get(r.trip_id.as_str()) else {
            bail!("results has trip `{}` that trips.csv does not", r.trip_id);
        };
        let Some(persona) = persona_by_id.get(trip.persona_id.as_str()) else {
            bail!("trip `{}` belongs to unknown persona `{}`", trip.trip_id, trip.persona_id);
        };
        let detailed = r.completed && r.breakdown_sampled;
        for (dim, value) in dimensions(persona) {
            let a = accs.entry((dim.to_string(), value)).or_default();
            a.trips += 1;
            if r.completed {
                a.completed += 1;
                a.total_sum += r.total_minutes.unwrap_or(0.0);
            }
            if detailed {
                a.detail_n += 1;
                a.wait_sum += r.wait_minutes.unwrap_or(0.0);
                a.in_vehicle_sum += r.in_vehicle_minutes.unwrap_or(0.0);
                a.walk_sum += r.walk_minutes.unwrap_or(0.0);
                a.transfers_sum += r.transfers.unwrap_or(0.0);
            }
            if trip.required {
                a.required += 1;
                if !r.completed {
                    a.required_failed += 1;
                }
            }
        }
        let h = hours
            .entry((enum_name(&persona.group), day_type(trip.day), departure_hour(&trip.depart_time)?))
            .or_default();
        h.trips += 1;
        if !r.completed {
            h.failed += 1;
        }
        if detailed {
            h.wait_n += 1;
            h.wait_sum += r.wait_minutes.unwrap_or(0.0);
        }
    }

    Ok(Replicate {
        seed,
        groups: accs.iter().map(|(k, a)| (k.clone(), finish(a))).collect(),
        hours,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::*;

    fn persona(id: &str, group: Group, hood: &str, tenure: Tenure) -> Persona {
        Persona {
            persona_id: id.into(),
            group,
            tenure,
            age_band: AgeBand::A25To34,
            income_band: IncomeBand::Mid,
            has_car: false,
            weight: 100.0,
            neighborhood: hood.into(),
            home_id: "h1".into(),
            home_lat: 42.0,
            home_lon: -83.0,
            anchor_id: None,
            anchor_type: AnchorType::None,
            anchor_lat: None,
            anchor_lon: None,
            shift_pattern: PersonaShift::StandardDay,
        }
    }

    fn trip(id: &str, persona: &str, day: Day, time: &str, required: bool) -> Trip {
        Trip {
            trip_id: id.into(),
            persona_id: persona.into(),
            day,
            depart_time: time.into(),
            purpose: if required { Purpose::Work } else { Purpose::Shopping },
            origin_lat: 0.0,
            origin_lon: 0.0,
            dest_lat: 0.0,
            dest_lon: 0.0,
            dest_id: "x".into(),
            required,
            due_time: required.then(|| "09:00".to_string()),
        }
    }

    fn result(id: &str, completed: bool, total: f64, detail: Option<(f64, f64, f64, f64)>) -> TripResult {
        TripResult {
            trip_id: id.into(),
            completed,
            total_minutes: completed.then_some(total),
            breakdown_sampled: detail.is_some(),
            wait_minutes: detail.map(|d| d.0),
            in_vehicle_minutes: detail.map(|d| d.1),
            walk_minutes: detail.map(|d| d.2),
            transfers: detail.map(|d| d.3),
            ..Default::default()
        }
    }

    fn sample() -> Replicate {
        let personas = vec![
            persona("p1", Group::Undergraduate, "campus", Tenure::Dorm),
            persona("p2", Group::Undergraduate, "campus", Tenure::Dorm),
            persona("p3", Group::Retired, "burns_park", Tenure::Own),
        ];
        let trips = vec![
            trip("t1", "p1", Day::Mon, "08:05", true),
            trip("t2", "p1", Day::Sat, "08:50", false),
            trip("t3", "p2", Day::Mon, "08:30", true),
            trip("t4", "p3", Day::Tue, "14:00", false),
        ];
        let results = vec![
            result("t1", true, 20.0, Some((4.0, 10.0, 6.0, 1.0))),
            result("t2", true, 40.0, None),
            result("t3", false, 0.0, None),
            result("t4", true, 10.0, Some((0.0, 0.0, 10.0, 0.0))),
        ];
        aggregate(42, &results, &personas, &trips).unwrap()
    }

    fn g(rep: &Replicate, dim: &str, value: &str) -> RepMetrics {
        rep.groups[&(dim.to_string(), value.to_string())].clone()
    }

    #[test]
    fn counts_personas_and_computes_means_over_the_right_trips() {
        let r = sample();
        let u = g(&r, "group", "undergraduate");
        assert_eq!(u.n_personas, 2.0);
        assert_eq!(u.total_minutes, Some(30.0), "mean of the two completed trips");
        assert_eq!(u.failure_rate, Some(1.0 / 3.0));
        assert_eq!(u.required_failure_rate, Some(0.5), "t3 is a failed required trip");
        // only t1 has a breakdown
        assert_eq!(u.wait_minutes, Some(4.0));
        assert_eq!(u.transfers, Some(1.0));
    }

    #[test]
    fn a_group_with_no_required_or_no_detailed_trips_has_none() {
        let r = sample();
        let retired = g(&r, "group", "retired");
        assert_eq!(retired.required_failure_rate, None);
        assert_eq!(retired.walk_minutes, Some(10.0));
        let own = g(&r, "tenure", "own");
        assert_eq!(own.total_minutes, Some(10.0));
    }

    #[test]
    fn every_persona_dimension_is_present() {
        let r = sample();
        for dim in DIMENSIONS {
            assert!(r.groups.keys().any(|(d, _)| d == dim), "{dim}");
        }
        assert!(r.groups.contains_key(&("neighborhood".into(), "campus".into())));
        assert!(r.groups.contains_key(&("age_band".into(), "25_34".into())));
    }

    #[test]
    fn hours_split_weekday_and_weekend_by_departure_hour() {
        let r = sample();
        let sat = &r.hours[&("undergraduate".to_string(), "weekend", 8)];
        assert_eq!((sat.trips, sat.failed), (1, 0));
        let mon8 = &r.hours[&("undergraduate".to_string(), "weekday", 8)];
        assert_eq!((mon8.trips, mon8.failed), (2, 1));
        assert_eq!((mon8.wait_n, mon8.wait_sum), (1, 4.0));
    }

    #[test]
    fn unknown_trip_is_an_error_not_a_silent_skip() {
        let personas = vec![persona("p1", Group::Retired, "x", Tenure::Own)];
        let err = aggregate(1, &[result("nope", true, 1.0, None)], &personas, &[]);
        assert!(err.is_err());
    }
}
