//! Reads `trip_results.parquet` (SCHEMA section 4.1), one run at a time.

use std::fs::File;
use std::path::Path;

use anyhow::{Context, Result};
use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::record::Field;

#[derive(Debug, Clone, Default)]
pub struct TripResult {
    pub trip_id: String,
    pub completed: bool,
    pub walk_only: bool,
    pub total_minutes: Option<f64>,
    pub wait_minutes: Option<f64>,
    pub in_vehicle_minutes: Option<f64>,
    pub walk_minutes: Option<f64>,
    pub transfers: Option<f64>,
    pub breakdown_sampled: bool,
    /// Delay model output (planned required trips in the detailed sample only)
    pub p_on_time: Option<f64>,
    pub delay_p95_minutes: Option<f64>,
    pub scheduled_minutes: Option<f64>,
}

fn number(field: &Field) -> Option<f64> {
    match field {
        Field::Double(v) => Some(*v),
        Field::Float(v) => Some(f64::from(*v)),
        Field::Long(v) => Some(*v as f64),
        Field::Int(v) => Some(f64::from(*v)),
        Field::Short(v) => Some(f64::from(*v)),
        Field::Byte(v) => Some(f64::from(*v)),
        _ => None, // Null
    }
    .filter(|v| v.is_finite()) // NaN is how pandas wrote a missing float
}

pub fn read_trip_results(path: &Path) -> Result<Vec<TripResult>> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let reader = SerializedFileReader::new(file)
        .with_context(|| format!("reading parquet {}", path.display()))?;
    let mut out = Vec::with_capacity(reader.metadata().file_metadata().num_rows() as usize);
    for row in reader.get_row_iter(None)? {
        let row = row?;
        let mut r = TripResult::default();
        for (name, field) in row.get_column_iter() {
            match (name.as_str(), field) {
                ("trip_id", Field::Str(s)) => r.trip_id = s.clone(),
                ("completed", Field::Bool(b)) => r.completed = *b,
                ("walk_only", Field::Bool(b)) => r.walk_only = *b,
                ("breakdown_sampled", Field::Bool(b)) => r.breakdown_sampled = *b,
                ("total_minutes", f) => r.total_minutes = number(f),
                ("wait_minutes", f) => r.wait_minutes = number(f),
                ("in_vehicle_minutes", f) => r.in_vehicle_minutes = number(f),
                ("walk_minutes", f) => r.walk_minutes = number(f),
                ("transfers", f) => r.transfers = number(f),
                ("p_on_time", f) => r.p_on_time = number(f),
                ("delay_p95_minutes", f) => r.delay_p95_minutes = number(f),
                ("scheduled_minutes", f) => r.scheduled_minutes = number(f),
                _ => {}
            }
        }
        out.push(r);
    }
    Ok(out)
}
