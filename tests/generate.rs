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

#[test]
fn trips_follow_the_routine() {
    let (_, personas, trips) = big_sample();
    let by_persona: BTreeMap<&str, &Persona> =
        personas.iter().map(|p| (p.persona_id.as_str(), p)).collect();
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for t in &trips {
        *counts.entry(t.persona_id.as_str()).or_default() += 1;
        // HH:MM, 24-hour
        let (h, m) = t.depart_time.split_once(':').unwrap();
        assert!(h.parse::<u32>().unwrap() < 24 && m.parse::<u32>().unwrap() < 60);
        assert_eq!(t.required, matches!(t.purpose, Purpose::Work | Purpose::Class));
    }
    for p in &personas {
        let n = counts.get(p.persona_id.as_str()).copied().unwrap_or(0);
        if p.group == Group::Retired {
            assert_eq!(n, 0, "retired personas have no v1 trips");
        } else {
            assert_eq!(n, 10, "5 weekdays x (out + return)");
        }
    }
    // Night workers: out in the afternoon, return the next morning.
    let night = trips.iter().find(|t| {
        by_persona[t.persona_id.as_str()].shift_pattern == PersonaShift::Night
            && t.purpose == Purpose::Work
    });
    let night = night.expect("a night worker exists");
    assert!(night.depart_time.as_str() >= "16:00" && night.depart_time.as_str() <= "17:00");
    let back = trips
        .iter()
        .find(|t| t.persona_id == night.persona_id && t.purpose == Purpose::ReturnHome)
        .unwrap();
    assert!(back.depart_time.as_str() >= "09:00" && back.depart_time.as_str() <= "10:00");
}
