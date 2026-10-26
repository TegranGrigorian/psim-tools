//! Weekly routines: persona -> explicit trip rows (outbound and return).
//!
//! Every persona with an anchor gets a weekday work/class routine. Every
//! persona, including retired ones, also gets discretionary shopping trips and
//! occasional medical trips (round trips from home to a POI). Times are
//! *desired departures*; the engine finds the itinerary. All numbers marked
//! ASSUMPTION are placeholders to refine with fieldwork and survey data.

use anyhow::{Result, bail};
use rand::distr::weighted::WeightedIndex;
use rand::{Rng, RngExt};

use crate::schema::{
    AnchorType, Day, Group, Persona, PersonaShift, Poi, PoiCategory, Purpose, Trip,
};

const WEEKDAYS: [Day; 5] = [Day::Mon, Day::Tue, Day::Wed, Day::Thu, Day::Fri];

// ASSUMPTIONS, work and class routine. Minutes after midnight.
// Workers get one departure time per direction for the whole week, jittered
// within a one-hour window in 5-minute steps.
const DAY_OUT: u32 = 7 * 60 + 30; // arrive by ~09:00
const DAY_BACK: u32 = 17 * 60; // shift ends 17:00
const NIGHT_OUT: u32 = 15 * 60 + 30; // leave 15:30-16:30 for the 17:00 start
const NIGHT_BACK: u32 = 9 * 60; // shift ends 09:00, next calendar day
const JITTER_STEPS: u32 = 12; // 0..=12 steps of 5 minutes = one hour
// Shift starts: 09:00 for the day shift, 17:00 for the night shift.
const DAY_DUE: u32 = 9 * 60;
const NIGHT_DUE: u32 = 17 * 60;
// Students: a class starts on the hour, 08:00-15:00, a different one each day; the nominal
// departure is 30 minutes before; they stay 3-8 hours after the start.
const CLASS_START_FIRST: u32 = 8 * 60;
const CLASS_START_SLOTS: u32 = 8; // 08:00 .. 15:00
const CLASS_NOMINAL_LEAD: u32 = 30;
const CLASS_STAY_MIN_STEPS: u32 = 36;
const CLASS_STAY_MAX_STEPS: u32 = 96;
const LATEST_RETURN: u32 = 22 * 60;

// ASSUMPTIONS, discretionary trips.
// Shopping trips per week, per persona (everyone gets at least one).
const SHOP_MIN: u32 = 1;
const SHOP_MAX: u32 = 2;
// Chance of a medical trip in a given week.
const MEDICAL_P_RETIRED: f64 = 0.15;
const MEDICAL_P_OTHER: f64 = 0.08;
// Time spent at the destination, in 5-minute steps.
const SHOP_STAY_STEPS: (u32, u32) = (4, 18); // 20-90 min
const MEDICAL_STAY_STEPS: (u32, u32) = (12, 36); // 60-180 min
// POI choice weight = poi.weight / (1 + distance_km)^GRAVITY_EXPONENT, so people
// mostly go to nearby places, and big places pull harder.
const GRAVITY_EXPONENT: f64 = 2.0;

/// Days discretionary trips can happen: Mon-Fri plus Saturday. Sunday has no
/// service date in the config, so it is not generated in v1.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DayKind {
    Weekday,
    Saturday,
}

fn day_index(day: Day) -> u32 {
    match day {
        Day::Mon => 0,
        Day::Tue => 1,
        Day::Wed => 2,
        Day::Thu => 3,
        Day::Fri => 4,
        Day::Sat => 5,
        Day::Sun => 6,
    }
}

fn next_day(day: Day) -> Day {
    match day {
        Day::Mon => Day::Tue,
        Day::Tue => Day::Wed,
        Day::Wed => Day::Thu,
        Day::Thu => Day::Fri,
        Day::Fri => Day::Sat,
        Day::Sat => Day::Sun,
        Day::Sun => Day::Mon,
    }
}

fn hhmm(minutes: u32) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

fn jittered<R: Rng + ?Sized>(base: u32, rng: &mut R) -> u32 {
    base + 5 * rng.random_range(0..=JITTER_STEPS)
}

/// Window of departure times (minutes) when a persona can start a discretionary
/// trip, chosen so it doesn't collide with their work or class routine.
/// `None` means they don't make that kind of trip on that kind of day.
fn free_window(p: &Persona, kind: DayKind, category: PoiCategory) -> Option<(u32, u32)> {
    let retired = p.group == Group::Retired;
    let night = p.shift_pattern == PersonaShift::Night;
    let student = p.shift_pattern == PersonaShift::Student;
    let h = |hours: u32, mins: u32| hours * 60 + mins;
    // Night workers get home from Friday's shift around 09:00-10:00 plus travel.
    let sat_start = |start: u32| if night { start.max(h(11, 0)) } else { start };
    match (category, kind) {
        (PoiCategory::Shopping, DayKind::Saturday) => Some((sat_start(h(10, 0)), h(17, 0))),
        (PoiCategory::Medical, DayKind::Saturday) => Some((sat_start(h(9, 0)), h(12, 0))),
        // Students only make discretionary trips on Saturday (class hours vary).
        (_, DayKind::Weekday) if student => None,
        (PoiCategory::Shopping, DayKind::Weekday) => {
            if retired {
                Some((h(9, 0), h(16, 0)))
            } else if night {
                Some((h(10, 30), h(13, 30))) // after sleeping; home by 15:00 for a 15:30 departure
            } else {
                Some((h(19, 30), h(21, 0))) // after getting home from work
            }
        }
        // Clinics are open weekday daytime; day workers go on Saturday instead.
        // Night workers must be home again (stay up to 3h) before leaving at 15:30.
        (PoiCategory::Medical, DayKind::Weekday) => {
            if retired {
                Some((h(9, 0), h(15, 0)))
            } else if night {
                Some((h(10, 30), h(12, 0)))
            } else {
                None
            }
        }
        (PoiCategory::Social, _) => None,
    }
}

/// Approximate distance in km (flat-earth, fine at city scale).
fn distance_km(a: (f64, f64), b: (f64, f64)) -> f64 {
    let dy = (a.0 - b.0) * 111.2;
    let dx = (a.1 - b.1) * 111.2 * ((a.0 + b.0) / 2.0).to_radians().cos();
    (dx * dx + dy * dy).sqrt()
}

fn pick_poi<'a, R: Rng + ?Sized>(
    pois: &[&'a Poi],
    home: (f64, f64),
    rng: &mut R,
) -> Result<&'a Poi> {
    let dist = WeightedIndex::new(pois.iter().map(|p| {
        p.weight / (1.0 + distance_km(home, (p.lat, p.lon))).powf(GRAVITY_EXPONENT)
    }))?;
    Ok(pois[rng.sample(&dist)])
}

/// Trips under construction for one persona; ids are assigned after sorting.
struct PersonaTrips<'a> {
    persona: &'a Persona,
    trips: Vec<Trip>,
}

impl PersonaTrips<'_> {
    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        day: Day,
        minutes: u32,
        purpose: Purpose,
        from: (f64, f64),
        to: (f64, f64),
        dest_id: &str,
        due: Option<u32>,
    ) {
        self.trips.push(Trip {
            trip_id: String::new(),
            persona_id: self.persona.persona_id.clone(),
            day,
            depart_time: hhmm(minutes),
            purpose,
            origin_lat: from.0,
            origin_lon: from.1,
            dest_lat: to.0,
            dest_lon: to.1,
            dest_id: dest_id.to_string(),
            required: matches!(purpose, Purpose::Work | Purpose::Class),
            due_time: due.map(hhmm),
        });
    }
}

fn anchor_trips<R: Rng + ?Sized>(t: &mut PersonaTrips<'_>, rng: &mut R) {
    let p = t.persona;
    if p.anchor_type == AnchorType::None {
        return;
    }
    let (Some(anchor_id), Some(alat), Some(alon)) =
        (p.anchor_id.as_deref(), p.anchor_lat, p.anchor_lon)
    else {
        return;
    };
    let home = (p.home_lat, p.home_lon);
    let anchor = (alat, alon);

    match p.shift_pattern {
        PersonaShift::Student => {
            for day in WEEKDAYS {
                let due = CLASS_START_FIRST + 60 * rng.random_range(0..CLASS_START_SLOTS);
                let stay = 5 * rng.random_range(CLASS_STAY_MIN_STEPS..=CLASS_STAY_MAX_STEPS);
                let back = (due + stay).min(LATEST_RETURN);
                t.push(day, due - CLASS_NOMINAL_LEAD, Purpose::Class, home, anchor, anchor_id, Some(due));
                t.push(day, back, Purpose::ReturnHome, anchor, home, &p.home_id, None);
            }
        }
        shift => {
            let (out_base, back_base, back_next_day, due) = match shift {
                PersonaShift::Night => (NIGHT_OUT, NIGHT_BACK, true, NIGHT_DUE),
                // v1 has two shifts; reserved shifts fall back to the day routine.
                _ => (DAY_OUT, DAY_BACK, false, DAY_DUE),
            };
            let leave = jittered(out_base, rng);
            let back = jittered(back_base, rng);
            for day in WEEKDAYS {
                let back_day = if back_next_day { next_day(day) } else { day };
                t.push(day, leave, Purpose::Work, home, anchor, anchor_id, Some(due));
                t.push(back_day, back, Purpose::ReturnHome, anchor, home, &p.home_id, None);
            }
        }
    }
}

/// Days of the week (Mon-Fri, Sat) on which this persona can take a trip of
/// this category.
fn candidate_days(p: &Persona, category: PoiCategory) -> Vec<(Day, u32, u32)> {
    let mut days = Vec::new();
    for day in WEEKDAYS {
        if let Some((a, b)) = free_window(p, DayKind::Weekday, category) {
            days.push((day, a, b));
        }
    }
    if let Some((a, b)) = free_window(p, DayKind::Saturday, category) {
        days.push((Day::Sat, a, b));
    }
    days
}

/// Adds one round trip home -> POI -> home. Returns the minute the persona is
/// back home again.
fn discretionary_trip<R: Rng + ?Sized>(
    t: &mut PersonaTrips<'_>,
    category: PoiCategory,
    pois: &[&Poi],
    day: Day,
    window: (u32, u32),
    rng: &mut R,
) -> Result<u32> {
    let p = t.persona;
    let home = (p.home_lat, p.home_lon);
    let poi = pick_poi(pois, home, rng)?;
    let leave = window.0 + 5 * rng.random_range(0..=(window.1 - window.0) / 5);
    let (lo, hi) = match category {
        PoiCategory::Medical => MEDICAL_STAY_STEPS,
        _ => SHOP_STAY_STEPS,
    };
    let back = leave + 5 * rng.random_range(lo..=hi);
    let purpose = match category {
        PoiCategory::Medical => Purpose::Medical,
        PoiCategory::Social => Purpose::Social,
        PoiCategory::Shopping => Purpose::Shopping,
    };
    t.push(day, leave, purpose, home, (poi.lat, poi.lon), &poi.poi_id, None);
    t.push(day, back, Purpose::ReturnHome, (poi.lat, poi.lon), home, &p.home_id, None);
    Ok(back)
}

fn discretionary_trips<R: Rng + ?Sized>(
    t: &mut PersonaTrips<'_>,
    shopping: &[&Poi],
    medical: &[&Poi],
    rng: &mut R,
) -> Result<()> {
    // Medical first: at most one trip per week.
    let p_medical = if t.persona.group == Group::Retired {
        MEDICAL_P_RETIRED
    } else {
        MEDICAL_P_OTHER
    };
    let mut medical_visit: Option<(Day, u32)> = None; // (day, minute back home)
    if rng.random_bool(p_medical) {
        let days = candidate_days(t.persona, PoiCategory::Medical);
        if !days.is_empty() {
            let (day, a, b) = days[rng.random_range(0..days.len())];
            let back = discretionary_trip(t, PoiCategory::Medical, medical, day, (a, b), rng)?;
            medical_visit = Some((day, back));
        }
    }

    // Shopping: SHOP_MIN..=SHOP_MAX trips, each on a different available day. On
    // the medical day, shopping can only start after the persona is home again.
    let mut days = candidate_days(t.persona, PoiCategory::Shopping);
    if let Some((medical_day, back)) = medical_visit {
        days.retain_mut(|(day, a, b)| {
            if *day == medical_day {
                *a = (*a).max(back + 15);
            }
            *a <= *b
        });
    }
    let n = rng.random_range(SHOP_MIN..=SHOP_MAX).min(days.len() as u32);
    for _ in 0..n {
        let (day, a, b) = days.swap_remove(rng.random_range(0..days.len()));
        discretionary_trip(t, PoiCategory::Shopping, shopping, day, (a, b), rng)?;
    }
    Ok(())
}

pub fn generate_trips<R: Rng + ?Sized>(
    personas: &[Persona],
    pois: &[Poi],
    rng: &mut R,
) -> Result<Vec<Trip>> {
    let shopping: Vec<&Poi> = pois.iter().filter(|p| p.category == PoiCategory::Shopping).collect();
    let medical: Vec<&Poi> = pois.iter().filter(|p| p.category == PoiCategory::Medical).collect();
    if shopping.is_empty() {
        bail!("pois.csv has no shopping POIs");
    }
    if medical.is_empty() {
        bail!("pois.csv has no medical POIs");
    }

    let mut all = Vec::new();
    for persona in personas {
        let mut t = PersonaTrips { persona, trips: Vec::new() };
        anchor_trips(&mut t, rng);
        discretionary_trips(&mut t, &shopping, &medical, rng)?;
        // Chronological within the persona (a night return lands on the next day).
        t.trips.sort_by_key(|trip| (day_index(trip.day), trip.depart_time.clone()));
        all.extend(t.trips);
    }
    for (i, trip) in all.iter_mut().enumerate() {
        trip.trip_id = format!("t{:06}", i + 1);
    }
    Ok(all)
}
