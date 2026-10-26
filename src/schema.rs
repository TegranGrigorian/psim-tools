//! Row types for the data contract in docs/SCHEMA.md.
//! Change SCHEMA.md first, then this file.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: &str = "0.1";

// ---------------------------------------------------------------------------
// Enums (lowercase snake_case, unknown values are a deserialization error)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Tenure {
    Own,
    Rent,
    Dorm,
    OtherGroup,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BuildingType {
    SingleFamily,
    SmallMulti,
    LargeMulti,
    Dorm,
    Other,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IncomeBand {
    Low,
    Mid,
    High,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Sector {
    University,
    Healthcare,
    RetailFood,
    Government,
    EducationK12,
    IndustryTech,
    OtherService,
}

/// Shift pattern of a workplace (`workplaces.csv`).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ShiftPattern {
    StandardDay,
    Early,
    Evening,
    Night,
    Rotating,
}

/// Shift pattern of a persona (`personas.csv`): the workplace values plus
/// `student` and `none`.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PersonaShift {
    StandardDay,
    Early,
    Evening,
    Night,
    Rotating,
    Student,
    None,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CampusZone {
    Central,
    North,
    Medical,
    Athletic,
    Other,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    Undergraduate,
    Graduate,
    UniversityStaff,
    ResidentWorker,
    ShiftWorker,
    Retired,
}

impl Group {
    /// Fixed order, so anything that iterates groups is deterministic.
    pub const ALL: [Group; 6] = [
        Group::Undergraduate,
        Group::Graduate,
        Group::UniversityStaff,
        Group::ResidentWorker,
        Group::ShiftWorker,
        Group::Retired,
    ];

    /// The snake_case name used in files (and as `group_shares` keys).
    pub fn name(self) -> &'static str {
        match self {
            Group::Undergraduate => "undergraduate",
            Group::Graduate => "graduate",
            Group::UniversityStaff => "university_staff",
            Group::ResidentWorker => "resident_worker",
            Group::ShiftWorker => "shift_worker",
            Group::Retired => "retired",
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgeBand {
    #[serde(rename = "18_24")]
    A18To24,
    #[serde(rename = "25_34")]
    A25To34,
    #[serde(rename = "35_49")]
    A35To49,
    #[serde(rename = "50_64")]
    A50To64,
    #[serde(rename = "65_plus")]
    A65Plus,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AnchorType {
    Workplace,
    Campus,
    None,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Day {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PoiCategory {
    Shopping,
    Medical,
    Social,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    Work,
    Class,
    Shopping,
    Medical,
    Social,
    ReturnHome,
}

// ---------------------------------------------------------------------------
// Input tables (SCHEMA section 2)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Housing {
    pub housing_id: String,
    pub lat: f64,
    pub lon: f64,
    pub tenure: Tenure,
    pub building_type: BuildingType,
    /// Households or beds this row represents, >= 1.
    pub units: u32,
    pub neighborhood: String,
    /// 11-digit Census tract GEOID. Kept as a string to preserve leading zeros.
    pub tract_geoid: String,
    pub median_income_band: Option<IncomeBand>,
    /// Share of the tract's population aged 18-24 (ACS). Weights where students live.
    #[serde(default)]
    pub share_18_24: Option<f64>,
    /// Share of this tenure's households in the tract with no vehicle (ACS). Sets `has_car`.
    #[serde(default)]
    pub no_vehicle_share: Option<f64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Workplace {
    pub workplace_id: String,
    pub lat: f64,
    pub lon: f64,
    pub name: Option<String>,
    pub sector: Sector,
    /// Capacity weight (approximate number of jobs), >= 1.
    pub jobs: u32,
    pub shift_pattern: ShiftPattern,
    pub is_university: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct CampusLocation {
    pub campus_id: String,
    pub lat: f64,
    pub lon: f64,
    pub name: Option<String>,
    pub campus_zone: CampusZone,
    /// Relative share of students attending classes here, > 0.
    pub capacity_weight: f64,
}

/// Discretionary destination (shopping, medical, ...).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Poi {
    pub poi_id: String,
    pub lat: f64,
    pub lon: f64,
    pub name: Option<String>,
    pub category: PoiCategory,
    /// Relative attractiveness (size), > 0.
    pub weight: f64,
}

// ---------------------------------------------------------------------------
// Generated tables (SCHEMA section 3)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Manifest {
    pub schema_version: String,
    pub run_id: String,
    pub city_slug: String,
    pub scenario: String,
    /// Base seed actually used (may have come from the clock). Always recorded.
    pub base_seed: u64,
    /// `base_seed + replicate`.
    pub seed: u64,
    pub replicate: u64,
    pub n_personas: u32,
    pub psim_tools_version: String,
    /// RFC 3339 UTC timestamp.
    pub generated_at: String,
    /// File name -> "sha256:<hex>". BTreeMap keeps key order stable.
    pub input_hashes: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Persona {
    pub persona_id: String,
    pub group: Group,
    pub tenure: Tenure,
    pub age_band: AgeBand,
    pub income_band: IncomeBand,
    pub has_car: bool,
    /// Real people this persona stands for: `represented_population / n_personas`.
    pub weight: f64,
    pub neighborhood: String,
    pub home_id: String,
    pub home_lat: f64,
    pub home_lon: f64,
    /// Empty for `retired`.
    pub anchor_id: Option<String>,
    pub anchor_type: AnchorType,
    pub anchor_lat: Option<f64>,
    pub anchor_lon: Option<f64>,
    pub shift_pattern: PersonaShift,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Trip {
    pub trip_id: String,
    pub persona_id: String,
    pub day: Day,
    /// Desired departure, `HH:MM` 24-hour local time.
    pub depart_time: String,
    pub purpose: Purpose,
    pub origin_lat: f64,
    pub origin_lon: f64,
    pub dest_lat: f64,
    pub dest_lon: f64,
    pub dest_id: String,
    /// True for work/class: a failure counts as a missed obligation.
    pub required: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read<T: serde::de::DeserializeOwned>(csv_text: &str) -> Result<Vec<T>, csv::Error> {
        csv::Reader::from_reader(csv_text.as_bytes())
            .deserialize()
            .collect()
    }

    #[test]
    fn housing_valid_row_parses() {
        let text = "housing_id,lat,lon,tenure,building_type,units,neighborhood,tract_geoid,median_income_band\n\
                    h000001,42.28,-83.74,rent,small_multi,4,old_west_side,26161400100,mid\n";
        let rows: Vec<Housing> = read(text).unwrap();
        assert_eq!(rows[0].tenure, Tenure::Rent);
        assert_eq!(rows[0].building_type, BuildingType::SmallMulti);
        assert_eq!(rows[0].tract_geoid, "26161400100");
        assert_eq!(rows[0].median_income_band, Some(IncomeBand::Mid));
        assert_eq!(rows[0].share_18_24, None, "optional columns may be absent from the file");
    }

    #[test]
    fn housing_optional_data_columns_parse_when_present() {
        let text = "housing_id,lat,lon,tenure,building_type,units,neighborhood,tract_geoid,median_income_band,share_18_24,no_vehicle_share\n\
                    h1,42.28,-83.74,rent,large_multi,40,t1,26161400100,low,0.45,0.2\n";
        let rows: Vec<Housing> = read(text).unwrap();
        assert_eq!(rows[0].share_18_24, Some(0.45));
        assert_eq!(rows[0].no_vehicle_share, Some(0.2));
    }

    #[test]
    fn housing_optional_column_may_be_empty() {
        let text = "housing_id,lat,lon,tenure,building_type,units,neighborhood,tract_geoid,median_income_band\n\
                    h000002,42.28,-83.74,own,single_family,1,burns_park,26161400200,\n";
        let rows: Vec<Housing> = read(text).unwrap();
        assert_eq!(rows[0].median_income_band, None);
    }

    #[test]
    fn housing_bad_enum_is_rejected() {
        let text = "housing_id,lat,lon,tenure,building_type,units,neighborhood,tract_geoid,median_income_band\n\
                    h000003,42.28,-83.74,squatting,single_family,1,burns_park,26161400200,low\n";
        assert!(read::<Housing>(text).is_err());
    }

    #[test]
    fn age_band_uses_schema_spelling() {
        assert_eq!(serde_json::to_string(&AgeBand::A18To24).unwrap(), "\"18_24\"");
        assert_eq!(
            serde_json::from_str::<AgeBand>("\"65_plus\"").unwrap(),
            AgeBand::A65Plus
        );
        assert!(serde_json::from_str::<AgeBand>("\"A18To24\"").is_err());
    }

    #[test]
    fn persona_retired_has_empty_anchor() {
        let text = "persona_id,group,tenure,age_band,income_band,has_car,weight,neighborhood,home_id,home_lat,home_lon,anchor_id,anchor_type,anchor_lat,anchor_lon,shift_pattern\n\
                    p000001,retired,own,65_plus,mid,true,123.851,burns_park,h000002,42.28,-83.74,,none,,,none\n";
        let rows: Vec<Persona> = read(text).unwrap();
        assert_eq!(rows[0].anchor_id, None);
        assert_eq!(rows[0].anchor_lat, None);
        assert_eq!(rows[0].anchor_type, AnchorType::None);
        assert_eq!(rows[0].shift_pattern, PersonaShift::None);
    }

    #[test]
    fn trip_round_trips_through_csv() {
        let trip = Trip {
            trip_id: "t000001".into(),
            persona_id: "p000001".into(),
            day: Day::Mon,
            depart_time: "08:15".into(),
            purpose: Purpose::ReturnHome,
            origin_lat: 42.28,
            origin_lon: -83.74,
            dest_lat: 42.29,
            dest_lon: -83.73,
            dest_id: "h000002".into(),
            required: false,
        };
        let mut w = csv::Writer::from_writer(vec![]);
        w.serialize(&trip).unwrap();
        let bytes = w.into_inner().unwrap();
        let back: Vec<Trip> = read(std::str::from_utf8(&bytes).unwrap()).unwrap();
        assert_eq!(back[0], trip);
    }

    #[test]
    fn manifest_keys_serialize_in_sorted_order() {
        let mut hashes = BTreeMap::new();
        hashes.insert("workplaces.csv".to_string(), "sha256:b".to_string());
        hashes.insert("housing.csv".to_string(), "sha256:a".to_string());
        let m = Manifest {
            schema_version: SCHEMA_VERSION.into(),
            run_id: "baseline_s0042".into(),
            city_slug: "ann_arbor".into(),
            scenario: "baseline".into(),
            base_seed: 42,
            seed: 42,
            replicate: 0,
            n_personas: 1000,
            psim_tools_version: "0.1.0".into(),
            generated_at: "2026-10-25T14:03:00Z".into(),
            input_hashes: hashes,
        };
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.find("housing.csv").unwrap() < json.find("workplaces.csv").unwrap());
    }
}
