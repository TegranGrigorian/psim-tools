//! Weekly routines: persona -> explicit trip rows (outbound and return).
//!
//! v1 generates only `work`, `class` and `return_home` trips on weekdays.
//! Retired personas have no anchor, so they get no trips until discretionary
//! trips (`pois.csv`) exist. Times are *desired departures*; the engine finds
//! the itinerary.

use rand::{Rng, RngExt};

use crate::schema::{AnchorType, Day, Persona, PersonaShift, Purpose, Trip};

const WEEKDAYS: [Day; 5] = [Day::Mon, Day::Tue, Day::Wed, Day::Thu, Day::Fri];

// ASSUMPTIONS (placeholders, refine with fieldwork). Minutes after midnight.
// Workers get one departure time per direction for the whole week, jittered
// within a one-hour window in 5-minute steps.
const DAY_OUT: u32 = 7 * 60 + 30; // arrive by ~09:00
const DAY_BACK: u32 = 17 * 60; // shift ends 17:00
const NIGHT_OUT: u32 = 16 * 60; // arrive by ~17:00
const NIGHT_BACK: u32 = 9 * 60; // shift ends 09:00, next calendar day
const JITTER_STEPS: u32 = 12; // 0..=12 steps of 5 minutes = one hour
// Students: class departure varies by day, 07:30-14:00; they stay 3-8 hours.
const CLASS_OUT_EARLIEST: u32 = 7 * 60 + 30;
const CLASS_OUT_STEPS: u32 = 78;
const CLASS_STAY_MIN_STEPS: u32 = 36;
const CLASS_STAY_MAX_STEPS: u32 = 96;
const LATEST_RETURN: u32 = 22 * 60;

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

struct TripWriter {
    trips: Vec<Trip>,
}

impl TripWriter {
    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        persona: &Persona,
        day: Day,
        minutes: u32,
        purpose: Purpose,
        from: (f64, f64),
        to: (f64, f64),
        dest_id: &str,
    ) {
        let n = self.trips.len() + 1;
        self.trips.push(Trip {
            trip_id: format!("t{n:06}"),
            persona_id: persona.persona_id.clone(),
            day,
            depart_time: hhmm(minutes),
            purpose,
            origin_lat: from.0,
            origin_lon: from.1,
            dest_lat: to.0,
            dest_lon: to.1,
            dest_id: dest_id.to_string(),
            required: matches!(purpose, Purpose::Work | Purpose::Class),
        });
    }
}

pub fn generate_trips<R: Rng + ?Sized>(personas: &[Persona], rng: &mut R) -> Vec<Trip> {
    let mut out = TripWriter { trips: Vec::new() };
    for p in personas {
        if p.anchor_type == AnchorType::None {
            continue;
        }
        let (Some(anchor_id), Some(alat), Some(alon)) =
            (p.anchor_id.as_deref(), p.anchor_lat, p.anchor_lon)
        else {
            continue;
        };
        let home = (p.home_lat, p.home_lon);
        let anchor = (alat, alon);

        match p.shift_pattern {
            PersonaShift::Student => {
                for day in WEEKDAYS {
                    let leave = CLASS_OUT_EARLIEST + 5 * rng.random_range(0..=CLASS_OUT_STEPS);
                    let stay =
                        5 * rng.random_range(CLASS_STAY_MIN_STEPS..=CLASS_STAY_MAX_STEPS);
                    let back = (leave + stay).min(LATEST_RETURN);
                    out.push(p, day, leave, Purpose::Class, home, anchor, anchor_id);
                    out.push(p, day, back, Purpose::ReturnHome, anchor, home, &p.home_id);
                }
            }
            shift => {
                let (out_base, back_base, back_next_day) = match shift {
                    PersonaShift::Night => (NIGHT_OUT, NIGHT_BACK, true),
                    // v1 has two shifts; reserved shifts fall back to the day routine.
                    _ => (DAY_OUT, DAY_BACK, false),
                };
                let leave = jittered(out_base, rng);
                let back = jittered(back_base, rng);
                for day in WEEKDAYS {
                    let back_day = if back_next_day { next_day(day) } else { day };
                    out.push(p, day, leave, Purpose::Work, home, anchor, anchor_id);
                    out.push(p, back_day, back, Purpose::ReturnHome, anchor, home, &p.home_id);
                }
            }
        }
    }
    out.trips
}
