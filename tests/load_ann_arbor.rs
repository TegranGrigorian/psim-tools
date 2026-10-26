use std::path::PathBuf;

use psim_tools::config::{CityConfig, Config};
use psim_tools::io::CityInputs;

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/data")
}

#[test]
fn ann_arbor_config_loads() {
    let cfg = CityConfig::load_city(&data_dir(), "ann_arbor").unwrap();
    assert_eq!(cfg.city_slug, "ann_arbor");
    assert!(cfg.persona_weight() > 1.0);
}

#[test]
fn ann_arbor_inputs_load() {
    let inputs = CityInputs::load(&data_dir().join("ann_arbor")).unwrap();
    assert!(!inputs.housing.is_empty());
    assert!(!inputs.workplaces.is_empty());
    assert!(!inputs.campus.is_empty());
}

#[test]
fn tool_config_loads() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tools_config.json");
    let cfg = Config::load(&path).unwrap();
    assert!(cfg.seed > 0);
}
