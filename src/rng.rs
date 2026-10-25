//! Seed handling. Every run's base seed is resolved once, then recorded in
//! `manifest.json`, so any run can be reproduced.

use std::time::{SystemTime, UNIX_EPOCH};

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

use crate::config::CityConfig;

/// Where a base seed came from, for logging and the manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedSource {
    Cli,
    Config,
    Clock,
}

/// Resolution order: CLI flag, then `config.json`, then the clock (unix seconds).
pub fn resolve_base_seed(cli: Option<u64>, city: &CityConfig) -> (u64, SeedSource) {
    resolve_with_clock(cli, city, clock_seed)
}

fn resolve_with_clock(
    cli: Option<u64>,
    city: &CityConfig,
    clock: impl FnOnce() -> u64,
) -> (u64, SeedSource) {
    if let Some(seed) = cli {
        (seed, SeedSource::Cli)
    } else if let Some(seed) = city.base_seed {
        (seed, SeedSource::Config)
    } else {
        (clock(), SeedSource::Clock)
    }
}

fn clock_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before 1970")
        .as_secs()
}

/// Seed for replicate `i`: `base_seed + i`.
pub fn replicate_seed(base_seed: u64, replicate: u64) -> u64 {
    base_seed + replicate
}

/// One RNG per replicate. Pass `&mut` down explicitly; never use a global RNG.
pub fn replicate_rng(base_seed: u64, replicate: u64) -> ChaCha8Rng {
    ChaCha8Rng::seed_from_u64(replicate_seed(base_seed, replicate))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;

    fn city(base_seed: Option<u64>) -> CityConfig {
        let text = format!(
            r#"{{
            "city_slug": "x", "display_name": "X", "timezone": "America/Detroit",
            "bbox": [-83.85, 42.20, -83.62, 42.34],
            "population_size": 1000, "represented_population": 100000,
            "n_replicates": 100, "base_seed": {},
            "gtfs": {{ "baseline": ["raw/gtfs/a"] }},
            "osm": "raw/osm.pbf", "service_date": "2026-03-10",
            "departure_window_minutes": 30, "max_walk_minutes": 20, "max_trip_minutes": 120,
            "group_shares": {{ "retired": 1.0 }}
        }}"#,
            base_seed.map_or("null".to_string(), |s| s.to_string())
        );
        serde_json::from_str(&text).unwrap()
    }

    #[test]
    fn cli_beats_config_beats_clock() {
        let (s, src) = resolve_with_clock(Some(7), &city(Some(42)), || 999);
        assert_eq!((s, src), (7, SeedSource::Cli));
        let (s, src) = resolve_with_clock(None, &city(Some(42)), || 999);
        assert_eq!((s, src), (42, SeedSource::Config));
        let (s, src) = resolve_with_clock(None, &city(None), || 999);
        assert_eq!((s, src), (999, SeedSource::Clock));
    }

    #[test]
    fn replicate_seeds_follow_base_plus_index() {
        assert_eq!(replicate_seed(42, 0), 42);
        assert_eq!(replicate_seed(42, 99), 141);
    }

    #[test]
    fn same_seed_same_stream_different_seed_differs() {
        let mut r1 = replicate_rng(42, 3);
        let mut r2 = replicate_rng(42, 3);
        let mut r3 = replicate_rng(42, 4);
        let (x1, x2, x3) = (r1.next_u64(), r2.next_u64(), r3.next_u64());
        assert_eq!(x1, x2);
        assert_ne!(x1, x3);
    }
}
