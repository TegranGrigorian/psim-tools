use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use psim_tools::config::CityConfig;
use psim_tools::generate::{GenerateOptions, generate_replicate, run};
use psim_tools::io::CityInputs;
use psim_tools::schema::{
    AgeBand, AnchorType, Group, Persona, PersonaShift, Purpose, Tenure, Trip,
};

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../data")
}

fn load() -> (CityConfig, CityInputs) {
    let cfg = CityConfig::load_city(&data_dir(), "ann_arbor").unwrap();
    let inputs = CityInputs::load(&data_dir().join("ann_arbor")).unwrap();
    (cfg, inputs)
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("psim_tools_test_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn opts(out: PathBuf, base_seed: u64, replicates: u64) -> GenerateOptions {
    GenerateOptions {
        data_dir: data_dir(),
        city: "ann_arbor".into(),
        scenario: "baseline".into(),
        base_seed: Some(base_seed),
        replicate: 0,
        replicates,
        out_dir: Some(out),
    }
}

#[test]
fn same_seed_gives_byte_identical_files() {
    let (a, b) = (scratch("det_a"), scratch("det_b"));
    run(&opts(a.clone(), 42, 1)).unwrap();
    run(&opts(b.clone(), 42, 1)).unwrap();
    for file in ["personas.csv", "trips.csv"] {
        let x = fs::read(a.join("baseline_s0042").join(file)).unwrap();
        let y = fs::read(b.join("baseline_s0042").join(file)).unwrap();
        assert!(!x.is_empty());
        assert_eq!(x, y, "{file} differs between identical runs");
    }
    let _ = fs::remove_dir_all(a);
    let _ = fs::remove_dir_all(b);
}

#[test]
fn different_seed_gives_different_output() {
    let (cfg, inputs) = load();
    let (p1, _) = generate_replicate(&cfg, &inputs, 42, 0).unwrap();
    let (p2, _) = generate_replicate(&cfg, &inputs, 43, 0).unwrap();
    assert_ne!(p1, p2);
}

#[test]
fn replicates_use_base_plus_index_and_manifest_records_seeds() {
    let out = scratch("replicates");
    let report = run(&opts(out.clone(), 100, 3)).unwrap();
    let seeds: Vec<u64> = report.runs.iter().map(|r| r.seed).collect();
    assert_eq!(seeds, vec![100, 101, 102]);
    let manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(out.join("baseline_s0101").join("manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["base_seed"], 100);
    assert_eq!(manifest["seed"], 101);
    assert_eq!(manifest["replicate"], 1);
    assert!(manifest["input_hashes"]["housing.csv"].as_str().unwrap().starts_with("sha256:"));
    let _ = fs::remove_dir_all(out);
}

#[test]
fn generate_replicate_is_deterministic() {
    let (cfg, inputs) = load();
    let a = generate_replicate(&cfg, &inputs, 42, 0).unwrap();
    let b = generate_replicate(&cfg, &inputs, 42, 0).unwrap();
    assert_eq!(a, b);
}

#[test]
fn personas_do_not_depend_on_scenario() {
    // Before/after comparisons need the same population in baseline and redesign.
    let (a, b) = (scratch("scen_a"), scratch("scen_b"));
    run(&opts(a.clone(), 42, 1)).unwrap();
    let mut redesign = opts(b.clone(), 42, 1);
    redesign.scenario = "redesign_v1".into();
    run(&redesign).unwrap();
    assert_eq!(
        fs::read(a.join("baseline_s0042/personas.csv")).unwrap(),
        fs::read(b.join("redesign_v1_s0042/personas.csv")).unwrap()
    );
    let _ = fs::remove_dir_all(a);
    let _ = fs::remove_dir_all(b);
}

#[test]
fn unknown_scenario_is_an_error() {
    let mut o = opts(scratch("bad_scen"), 42, 1);
    o.scenario = "nope".into();
    assert!(run(&o).is_err());
}

fn big_sample() -> (CityInputs, Vec<Persona>, Vec<Trip>) {
    let (mut cfg, inputs) = load();
    cfg.population_size = 20_000;
    cfg.represented_population = 200_000;
    let (personas, trips) = generate_replicate(&cfg, &inputs, 42, 0).unwrap();
    (inputs, personas, trips)
}

#[test]
fn group_shares_match_config_over_many_personas() {
    let (cfg, _) = load();
    let (_, personas, _) = big_sample();
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for p in &personas {
        *counts.entry(p.group.name()).or_default() += 1;
    }
    for g in Group::ALL {
        let observed = counts.get(g.name()).copied().unwrap_or(0) as f64 / personas.len() as f64;
        let expected = cfg.group_shares[g.name()];
        assert!(
            (observed - expected).abs() < 0.02,
            "{}: observed {observed:.3}, expected {expected:.3}",
            g.name()
        );
    }
}

#[test]
fn group_rules_hold() {
    let (inputs, personas, _) = big_sample();
    let work = |id: &str| inputs.workplaces.iter().find(|w| w.workplace_id == id).unwrap();
    let home = |id: &str| inputs.housing.iter().find(|h| h.housing_id == id).unwrap();
    for p in &personas {
        let h = home(p.home_id.as_str());
        assert_eq!(p.tenure, h.tenure);
        assert!((p.weight - 10.0).abs() < 1e-9);
        match p.group {
            Group::Undergraduate => {
                assert!(matches!(h.tenure, Tenure::Dorm | Tenure::Rent));
                assert_eq!(p.anchor_type, AnchorType::Campus);
                assert_eq!(p.shift_pattern, PersonaShift::Student);
                assert_eq!(p.age_band, AgeBand::A18To24);
            }
            Group::Graduate => {
                assert_eq!(h.tenure, Tenure::Rent);
                assert_eq!(p.anchor_type, AnchorType::Campus);
                assert!(matches!(p.age_band, AgeBand::A18To24 | AgeBand::A25To34));
            }
            Group::UniversityStaff => {
                assert_ne!(h.tenure, Tenure::Dorm);
                let w = work(p.anchor_id.as_deref().unwrap());
                assert!(w.is_university);
                assert_eq!(p.shift_pattern, PersonaShift::StandardDay);
            }
            Group::ResidentWorker => {
                let w = work(p.anchor_id.as_deref().unwrap());
                assert!(!w.is_university);
                assert_eq!(p.shift_pattern, PersonaShift::StandardDay);
            }
            Group::ShiftWorker => {
                let w = work(p.anchor_id.as_deref().unwrap());
                assert_ne!(w.shift_pattern, psim_tools::schema::ShiftPattern::StandardDay);
                assert_eq!(p.shift_pattern, PersonaShift::Night);
            }
            Group::Retired => {
                assert_eq!(p.anchor_id, None);
                assert_eq!(p.anchor_type, AnchorType::None);
                assert_eq!(p.age_band, AgeBand::A65Plus);
                assert_ne!(h.tenure, Tenure::Dorm);
            }
        }
    }
}

#[test]
fn dorm_residents_rarely_have_cars() {
    let (_, personas, _) = big_sample();
    let dorm: Vec<_> = personas.iter().filter(|p| p.tenure == Tenure::Dorm).collect();
    let owners = dorm.iter().filter(|p| p.has_car).count() as f64 / dorm.len() as f64;
    assert!(owners < 0.2, "dorm car share {owners:.2}");
}

fn is_anchor_trip(t: &Trip) -> bool {
    matches!(t.purpose, Purpose::Work | Purpose::Class)
}

#[test]
fn trip_times_are_valid_and_required_flag_matches_purpose() {
    let (_, _, trips) = big_sample();
    for t in &trips {
        let (h, m) = t.depart_time.split_once(':').unwrap();
        assert!(h.parse::<u32>().unwrap() < 24 && m.parse::<u32>().unwrap() < 60);
        assert_eq!(t.required, is_anchor_trip(t));
    }
    // ids are sequential and unique
    for (i, t) in trips.iter().enumerate() {
        assert_eq!(t.trip_id, format!("t{:06}", i + 1));
    }
}

#[test]
fn everyone_gets_work_or_class_and_shopping_trips() {
    let (_, personas, trips) = big_sample();
    let mut anchor: BTreeMap<&str, usize> = BTreeMap::new();
    let mut shopping: BTreeMap<&str, usize> = BTreeMap::new();
    for t in &trips {
        match t.purpose {
            Purpose::Work | Purpose::Class => *anchor.entry(t.persona_id.as_str()).or_default() += 1,
            Purpose::Shopping => *shopping.entry(t.persona_id.as_str()).or_default() += 1,
            _ => {}
        }
    }
    for p in &personas {
        let id = p.persona_id.as_str();
        if p.group == Group::Retired {
            assert_eq!(anchor.get(id), None, "retired have no anchor trips");
        } else {
            assert_eq!(anchor[id], 5, "one work/class trip per weekday");
        }
        let n = shopping.get(id).copied().unwrap_or(0);
        assert!((1..=2).contains(&n), "{id} has {n} shopping trips");
    }
}

#[test]
fn every_outbound_trip_has_a_matching_return_home() {
    let (_, personas, trips) = big_sample();
    let by_persona: BTreeMap<&str, &Persona> =
        personas.iter().map(|p| (p.persona_id.as_str(), p)).collect();
    // ids are sequential, so each persona's trips are contiguous and chronological
    let mut outbound = 0;
    let mut returns = 0;
    for t in &trips {
        if t.purpose == Purpose::ReturnHome {
            returns += 1;
            let p = by_persona[t.persona_id.as_str()];
            assert_eq!(t.dest_id, p.home_id);
            assert_eq!((t.dest_lat, t.dest_lon), (p.home_lat, p.home_lon));
        } else {
            outbound += 1;
            let p = by_persona[t.persona_id.as_str()];
            assert_eq!((t.origin_lat, t.origin_lon), (p.home_lat, p.home_lon));
        }
    }
    assert_eq!(outbound, returns);
}

#[test]
fn night_workers_leave_in_the_afternoon_and_return_next_morning() {
    let (_, personas, trips) = big_sample();
    let night: Vec<&str> = personas
        .iter()
        .filter(|p| p.shift_pattern == PersonaShift::Night)
        .map(|p| p.persona_id.as_str())
        .collect();
    assert!(!night.is_empty());
    let id = night[0];
    let work: Vec<&Trip> = trips
        .iter()
        .filter(|t| t.persona_id == id && t.purpose == Purpose::Work)
        .collect();
    assert_eq!(work.len(), 5);
    assert!(work.iter().all(|t| t.depart_time.as_str() >= "16:00" && t.depart_time.as_str() <= "17:00"));
    // Friday's shift returns on Saturday morning.
    let sat_back = trips.iter().find(|t| {
        t.persona_id == id
            && t.purpose == Purpose::ReturnHome
            && t.day == psim_tools::schema::Day::Sat
            && t.depart_time.as_str() < "10:01"
    });
    assert!(sat_back.is_some(), "Friday night shift should return on Saturday");
}

#[test]
fn discretionary_trips_stay_inside_their_windows() {
    let (_, personas, trips) = big_sample();
    let by_persona: BTreeMap<&str, &Persona> =
        personas.iter().map(|p| (p.persona_id.as_str(), p)).collect();
    use psim_tools::schema::Day;
    for t in trips.iter().filter(|t| matches!(t.purpose, Purpose::Shopping | Purpose::Medical)) {
        let p = by_persona[t.persona_id.as_str()];
        let time = t.depart_time.as_str();
        // destination ids encode the POI category
        match t.purpose {
            Purpose::Shopping => assert!(t.dest_id.starts_with('s')),
            _ => assert!(t.dest_id.starts_with('m')),
        }
        let within = |a: &str, b: &str| (a..=b).contains(&time);
        if t.day == Day::Sat {
            assert!(within("09:00", "17:00"));
            if p.shift_pattern == PersonaShift::Night {
                assert!(within("11:00", "17:00"), "night worker still commuting home at {time}");
            }
        } else {
            assert_ne!(p.shift_pattern, PersonaShift::Student, "students go on Saturday only");
            if p.group == Group::Retired {
                assert!(within("09:00", "16:00"));
            } else if p.shift_pattern == PersonaShift::Night {
                assert!(within("10:30", "14:00"));
            } else {
                assert_eq!(t.purpose, Purpose::Shopping, "day workers see clinics on Saturday");
                assert!(within("19:30", "21:00"));
            }
        }
    }
}

#[test]
fn retired_make_more_medical_trips_than_others() {
    let (_, personas, trips) = big_sample();
    let retired: BTreeMap<&str, bool> = personas
        .iter()
        .map(|p| (p.persona_id.as_str(), p.group == Group::Retired))
        .collect();
    let rate = |want_retired: bool| {
        let n = personas.iter().filter(|p| (p.group == Group::Retired) == want_retired).count();
        let m = trips
            .iter()
            .filter(|t| t.purpose == Purpose::Medical && retired[t.persona_id.as_str()] == want_retired)
            .count();
        m as f64 / n as f64
    };
    assert!(rate(true) > rate(false) * 1.4, "{} vs {}", rate(true), rate(false));
    assert!((rate(false) - 0.08).abs() < 0.02);
}

#[test]
fn shopping_prefers_nearby_pois() {
    let (inputs, personas, trips) = big_sample();
    let by_persona: BTreeMap<&str, &Persona> =
        personas.iter().map(|p| (p.persona_id.as_str(), p)).collect();
    let dist = |a: (f64, f64), b: (f64, f64)| {
        let dy = (a.0 - b.0) * 111.2;
        let dx = (a.1 - b.1) * 111.2 * a.0.to_radians().cos();
        (dx * dx + dy * dy).sqrt()
    };
    let shops: Vec<_> = inputs
        .pois
        .iter()
        .filter(|p| p.category == psim_tools::schema::PoiCategory::Shopping)
        .collect();
    let (mut chosen, mut any, mut n) = (0.0, 0.0, 0.0);
    for t in trips.iter().filter(|t| t.purpose == Purpose::Shopping) {
        let p = by_persona[t.persona_id.as_str()];
        chosen += dist((p.home_lat, p.home_lon), (t.dest_lat, t.dest_lon));
        any += shops.iter().map(|s| dist((p.home_lat, p.home_lon), (s.lat, s.lon))).sum::<f64>()
            / shops.len() as f64;
        n += 1.0;
    }
    assert!(chosen / n < 0.8 * any / n, "chosen {:.2} km vs uniform {:.2} km", chosen / n, any / n);
}

#[test]
fn night_workers_are_home_before_their_shift_starts() {
    let (_, personas, trips) = big_sample();
    let night: std::collections::BTreeSet<&str> = personas
        .iter()
        .filter(|p| p.shift_pattern == PersonaShift::Night)
        .map(|p| p.persona_id.as_str())
        .collect();
    // Weekday discretionary round trips must end before the 16:00 departure.
    // Trips are chronological per persona: an outbound is followed by its return.
    for pair in trips.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        let discretionary = matches!(a.purpose, Purpose::Shopping | Purpose::Medical);
        if discretionary
            && night.contains(a.persona_id.as_str())
            && a.day != psim_tools::schema::Day::Sat
        {
            assert_eq!(b.purpose, Purpose::ReturnHome);
            assert!(b.depart_time.as_str() < "16:00", "returns {} from {}", b.depart_time, a.dest_id);
        }
    }
}

#[test]
fn a_persona_is_never_in_two_places_at_once() {
    let (_, _, trips) = big_sample();
    // Within a persona's chronological list, every outbound trip must be home
    // again (its return_home) before the next outbound starts.
    let mut i = 0;
    while i < trips.len() {
        let id = &trips[i].persona_id;
        let mut j = i;
        while j < trips.len() && trips[j].persona_id == *id {
            j += 1;
        }
        let mine = &trips[i..j];
        let mut away = false;
        let mut last_day = None;
        for t in mine {
            match t.purpose {
                Purpose::ReturnHome => {
                    // a night shift's return arrives from the previous day's trip
                    assert!(away || t.day != psim_tools::schema::Day::Mon, "{id}: return without outbound");
                    away = false;
                }
                _ => {
                    assert!(!away || last_day != Some(t.day), "{id}: second trip on {:?} while away at {}", t.day, t.depart_time);
                    away = true;
                }
            }
            last_day = Some(t.day);
        }
        i = j;
    }
}

fn housing_with(cfg_dorm_share: Option<f64>, rent_share_18_24: impl Fn(&str) -> Option<f64>) -> (Vec<Persona>, CityInputs) {
    let (mut cfg, mut inputs) = load();
    cfg.population_size = 20_000;
    cfg.undergraduate_dorm_share = cfg_dorm_share;
    for h in &mut inputs.housing {
        h.share_18_24 = rent_share_18_24(&h.housing_id);
    }
    let (personas, _) = generate_replicate(&cfg, &inputs, 42, 0).unwrap();
    (personas, inputs)
}

#[test]
fn undergraduate_dorm_share_sets_the_dorm_fraction() {
    let (personas, _) = housing_with(Some(0.30), |_| None);
    let undergrads: Vec<_> = personas.iter().filter(|p| p.group == Group::Undergraduate).collect();
    let dorm = undergrads.iter().filter(|p| p.tenure == Tenure::Dorm).count() as f64 / undergrads.len() as f64;
    assert!((dorm - 0.30).abs() < 0.03, "dorm share {dorm:.3}");
    assert!(undergrads.iter().all(|p| matches!(p.tenure, Tenure::Dorm | Tenure::Rent)));
}

#[test]
fn rentals_near_young_adults_get_more_students() {
    // two sample rentals with equal-ish units: give one a much younger tract
    let (_, inputs) = load();
    let rentals: Vec<&str> = inputs.housing.iter().filter(|h| h.tenure == Tenure::Rent).map(|h| h.housing_id.as_str()).collect();
    let (young, old) = (rentals[0].to_string(), rentals[1].to_string());
    let units = |id: &str| f64::from(inputs.housing.iter().find(|h| h.housing_id == id).unwrap().units);
    let share = |id: &str| Some(if id == young { 0.9 } else if id == old { 0.05 } else { 0.3 });
    let (personas, _) = housing_with(Some(0.0), share);
    let count = |id: &str| personas.iter().filter(|p| p.group == Group::Undergraduate && p.home_id == id).count() as f64;
    let ratio = (count(&young) / units(&young)) / (count(&old) / units(&old));
    assert!(ratio > 8.0, "students per unit ratio young:old = {ratio:.1}, expected about 18");
}

#[test]
fn has_car_follows_the_acs_no_vehicle_share_when_present() {
    let (mut cfg, mut inputs) = load();
    cfg.population_size = 20_000;
    for h in &mut inputs.housing {
        h.no_vehicle_share = Some(0.25);
    }
    let (personas, _) = generate_replicate(&cfg, &inputs, 42, 0).unwrap();
    let non_dorm: Vec<_> = personas.iter().filter(|p| p.tenure != Tenure::Dorm).collect();
    let car = non_dorm.iter().filter(|p| p.has_car).count() as f64 / non_dorm.len() as f64;
    assert!((car - 0.75).abs() < 0.02, "car share {car:.3}");
}
