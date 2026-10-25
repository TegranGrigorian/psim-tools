//! Persona generation: draw a group, then a home and an anchor allowed for
//! that group, then the cross-cutting attributes. Rules are documented in
//! docs/SCHEMA.md (section 3.2).

use anyhow::{Result, bail};
use rand::distr::weighted::WeightedIndex;
use rand::{Rng, RngExt};

use crate::config::CityConfig;
use crate::io::CityInputs;
use crate::schema::{
    AgeBand, AnchorType, Group, Housing, IncomeBand, Persona, PersonaShift, ShiftPattern, Tenure,
    Workplace,
};

// ASSUMPTIONS (placeholders, replace with data when available).
// Chance that a persona's income band differs from their home tract's band.
const INCOME_NOISE: f64 = 0.20;
// Chance of owning a car by income band, e.g. from ACS vehicle availability by tract.
const CAR_LOW: f64 = 0.55;
const CAR_MID: f64 = 0.80;
const CAR_HIGH: f64 = 0.92;
// Dorm residents rarely have a car on campus.
const CAR_DORM: f64 = 0.10;

/// A set of candidate rows (indices into the input table) with a sampler.
struct Pool {
    indices: Vec<usize>,
    dist: WeightedIndex<f64>,
}

impl Pool {
    fn build<T>(
        items: &[T],
        keep: impl Fn(&T) -> bool,
        weight: impl Fn(&T) -> f64,
        label: &str,
    ) -> Result<Self> {
        let indices: Vec<usize> = (0..items.len()).filter(|&i| keep(&items[i])).collect();
        if indices.is_empty() {
            bail!("no rows available for pool `{label}`; check the input tables");
        }
        let dist = WeightedIndex::new(indices.iter().map(|&i| weight(&items[i])))?;
        Ok(Self { indices, dist })
    }

    fn pick<R: Rng + ?Sized>(&self, rng: &mut R) -> usize {
        self.indices[rng.sample(&self.dist)]
    }
}

/// Candidate homes and anchors per group, built once per replicate.
struct Pools {
    homes_undergrad: Option<Pool>,
    homes_grad: Option<Pool>,
    homes_other: Option<Pool>,
    campus: Option<Pool>,
    univ_staff: Option<Pool>,
    resident: Option<Pool>,
    shift: Option<Pool>,
}

impl Pools {
    /// Builds only the pools a group with a positive share needs.
    fn build(cfg: &CityConfig, inputs: &CityInputs) -> Result<Self> {
        let wants = |groups: &[Group]| {
            groups
                .iter()
                .any(|g| cfg.group_shares.get(g.name()).copied().unwrap_or(0.0) > 0.0)
        };
        let units = |h: &Housing| f64::from(h.units);
        let jobs = |w: &Workplace| f64::from(w.jobs);
        let opt = |needed: bool, pool: Result<Pool>| -> Result<Option<Pool>> {
            if needed { pool.map(Some) } else { Ok(None) }
        };

        let h = &inputs.housing;
        let w = &inputs.workplaces;
        use Group::*;
        Ok(Self {
            homes_undergrad: opt(
                wants(&[Undergraduate]),
                Pool::build(
                    h,
                    |x| matches!(x.tenure, Tenure::Dorm | Tenure::Rent),
                    units,
                    "undergraduate homes (dorm or rent)",
                ),
            )?,
            homes_grad: opt(
                wants(&[Graduate]),
                Pool::build(h, |x| x.tenure == Tenure::Rent, units, "graduate homes (rent)"),
            )?,
            homes_other: opt(
                wants(&[UniversityStaff, ResidentWorker, ShiftWorker, Retired]),
                Pool::build(h, |x| x.tenure != Tenure::Dorm, units, "non-student homes"),
            )?,
            campus: opt(
                wants(&[Undergraduate, Graduate]),
                Pool::build(&inputs.campus, |_| true, |c| c.capacity_weight, "campus locations"),
            )?,
            univ_staff: opt(
                wants(&[UniversityStaff]),
                Pool::build(
                    w,
                    |x| x.is_university && x.shift_pattern == ShiftPattern::StandardDay,
                    jobs,
                    "university standard_day workplaces",
                ),
            )?,
            resident: opt(
                wants(&[ResidentWorker]),
                Pool::build(
                    w,
                    |x| !x.is_university && x.shift_pattern == ShiftPattern::StandardDay,
                    jobs,
                    "non-university standard_day workplaces",
                ),
            )?,
            shift: opt(
                wants(&[ShiftWorker]),
                Pool::build(
                    w,
                    |x| x.shift_pattern != ShiftPattern::StandardDay,
                    jobs,
                    "non-standard_day workplaces",
                ),
            )?,
        })
    }
}

fn persona_shift(shift: ShiftPattern) -> PersonaShift {
    match shift {
        ShiftPattern::StandardDay => PersonaShift::StandardDay,
        ShiftPattern::Early => PersonaShift::Early,
        ShiftPattern::Evening => PersonaShift::Evening,
        ShiftPattern::Night => PersonaShift::Night,
        ShiftPattern::Rotating => PersonaShift::Rotating,
    }
}

fn age_band<R: Rng + ?Sized>(group: Group, rng: &mut R) -> AgeBand {
    match group {
        Group::Undergraduate => AgeBand::A18To24,
        Group::Graduate => {
            // Graduate students are 22-34.
            if rng.random_range(22..=34) <= 24 {
                AgeBand::A18To24
            } else {
                AgeBand::A25To34
            }
        }
        Group::Retired => AgeBand::A65Plus,
        // Working-age population, spread by years in each band (10, 15, 15).
        Group::UniversityStaff | Group::ResidentWorker | Group::ShiftWorker => {
            match rng.random_range(0..40) {
                0..10 => AgeBand::A25To34,
                10..25 => AgeBand::A35To49,
                _ => AgeBand::A50To64,
            }
        }
    }
}

/// Housing determines income: the home's tract band, with some noise.
fn income_band<R: Rng + ?Sized>(home: &Housing, rng: &mut R) -> IncomeBand {
    let base = home.median_income_band.unwrap_or(IncomeBand::Mid);
    if !rng.random_bool(INCOME_NOISE) {
        return base;
    }
    match base {
        IncomeBand::Low | IncomeBand::High => IncomeBand::Mid,
        IncomeBand::Mid => {
            if rng.random_bool(0.5) {
                IncomeBand::Low
            } else {
                IncomeBand::High
            }
        }
    }
}

fn has_car<R: Rng + ?Sized>(home: &Housing, income: IncomeBand, rng: &mut R) -> bool {
    let p = if home.tenure == Tenure::Dorm {
        CAR_DORM
    } else {
        match income {
            IncomeBand::Low => CAR_LOW,
            IncomeBand::Mid => CAR_MID,
            IncomeBand::High => CAR_HIGH,
        }
    };
    rng.random_bool(p)
}

/// Generates `cfg.population_size` personas for one replicate.
pub fn generate_personas<R: Rng + ?Sized>(
    cfg: &CityConfig,
    inputs: &CityInputs,
    rng: &mut R,
) -> Result<Vec<Persona>> {
    let pools = Pools::build(cfg, inputs)?;
    let group_dist = WeightedIndex::new(
        Group::ALL
            .iter()
            .map(|g| cfg.group_shares.get(g.name()).copied().unwrap_or(0.0)),
    )?;
    let weight = cfg.persona_weight();

    let mut personas = Vec::with_capacity(cfg.population_size as usize);
    for i in 0..cfg.population_size {
        let group = Group::ALL[rng.sample(&group_dist)];

        let (home_pool, is_student) = match group {
            Group::Undergraduate => (&pools.homes_undergrad, true),
            Group::Graduate => (&pools.homes_grad, true),
            _ => (&pools.homes_other, false),
        };
        let home = &inputs.housing[home_pool.as_ref().expect("pool built for group").pick(rng)];

        // Anchor: campus for students, a workplace for workers, none for retired.
        let (anchor_id, anchor_type, anchor_lat, anchor_lon, shift) = if is_student {
            let c = &inputs.campus[pools.campus.as_ref().expect("campus pool").pick(rng)];
            (
                Some(c.campus_id.clone()),
                AnchorType::Campus,
                Some(c.lat),
                Some(c.lon),
                PersonaShift::Student,
            )
        } else if group == Group::Retired {
            (None, AnchorType::None, None, None, PersonaShift::None)
        } else {
            let pool = match group {
                Group::UniversityStaff => &pools.univ_staff,
                Group::ResidentWorker => &pools.resident,
                _ => &pools.shift,
            };
            let w = &inputs.workplaces[pool.as_ref().expect("workplace pool").pick(rng)];
            (
                Some(w.workplace_id.clone()),
                AnchorType::Workplace,
                Some(w.lat),
                Some(w.lon),
                persona_shift(w.shift_pattern),
            )
        };

        let age = age_band(group, rng);
        let income = income_band(home, rng);
        let car = has_car(home, income, rng);

        personas.push(Persona {
            persona_id: format!("p{:06}", i + 1),
            group,
            tenure: home.tenure,
            age_band: age,
            income_band: income,
            has_car: car,
            weight,
            neighborhood: home.neighborhood.clone(),
            home_id: home.housing_id.clone(),
            home_lat: home.lat,
            home_lon: home.lon,
            anchor_id,
            anchor_type,
            anchor_lat,
            anchor_lon,
            shift_pattern: shift,
        });
    }
    Ok(personas)
}
