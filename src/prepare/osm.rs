//! Reads the GeoJSON `osmium export` makes of the points of interest we extract.

use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{Map, Value};

type Ring = Vec<(f64, f64)>; // (lon, lat)

#[derive(Debug, Clone)]
pub enum Geom {
    Point(f64, f64),
    /// Outer rings only; holes are ignored.
    Polygons(Vec<Ring>),
}

#[derive(Debug, Clone)]
pub struct Feature {
    pub id: String,
    pub tags: Map<String, Value>,
    pub geom: Geom,
}

impl Feature {
    pub fn tag(&self, key: &str) -> Option<&str> {
        self.tags.get(key)?.as_str()
    }

    /// The name with non-breaking spaces fixed.
    pub fn name(&self) -> Option<String> {
        self.tag("name").map(|n| n.replace('\u{a0}', " ").trim().to_string())
    }
}

fn ring(v: &Value) -> Option<Ring> {
    v.as_array()?
        .iter()
        .map(|p| Some((p.get(0)?.as_f64()?, p.get(1)?.as_f64()?)))
        .collect()
}

fn geometry(g: &Value) -> Option<Geom> {
    let c = &g["coordinates"];
    match g["type"].as_str()? {
        "Point" => Some(Geom::Point(c.get(0)?.as_f64()?, c.get(1)?.as_f64()?)),
        "Polygon" => Some(Geom::Polygons(vec![ring(c.get(0)?)?])),
        "MultiPolygon" => Some(Geom::Polygons(
            c.as_array()?.iter().filter_map(|poly| ring(poly.get(0)?)).collect(),
        )),
        _ => None,
    }
}

impl Geom {
    /// Mean of the outer ring's vertices (good enough for a building or a lot).
    pub fn centroid(&self) -> (f64, f64) {
        match self {
            Geom::Point(x, y) => (*x, *y),
            Geom::Polygons(rings) => {
                // A GeoJSON ring repeats its first point at the end; don't count it twice.
                let pts: Vec<&(f64, f64)> = rings
                    .iter()
                    .flat_map(|r| if r.len() > 1 && r.first() == r.last() { &r[..r.len() - 1] } else { &r[..] })
                    .collect();
                let n = pts.len().max(1) as f64;
                (pts.iter().map(|p| p.0).sum::<f64>() / n, pts.iter().map(|p| p.1).sum::<f64>() / n)
            }
        }
    }

    pub fn contains(&self, lon: f64, lat: f64) -> bool {
        match self {
            Geom::Point(..) => false,
            Geom::Polygons(rings) => rings.iter().any(|r| in_ring(r, lon, lat)),
        }
    }

    /// Footprint in square metres (outer rings, flat-earth projection).
    pub fn area_m2(&self) -> f64 {
        let Geom::Polygons(rings) = self else { return 0.0 };
        rings
            .iter()
            .map(|r| {
                let lat0 = r.iter().map(|p| p.1).sum::<f64>() / r.len().max(1) as f64;
                let (kx, ky) = (111_320.0 * lat0.to_radians().cos(), 110_540.0);
                let pts: Vec<(f64, f64)> = r.iter().map(|p| (p.0 * kx, p.1 * ky)).collect();
                let twice: f64 = (0..pts.len())
                    .map(|i| {
                        let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
                        a.0 * b.1 - b.0 * a.1
                    })
                    .sum();
                twice.abs() / 2.0
            })
            .sum()
    }

    pub fn is_polygon(&self) -> bool {
        matches!(self, Geom::Polygons(_))
    }
}

/// Ray casting: count how many edges a ray to the east crosses.
fn in_ring(ring: &Ring, x: f64, y: f64) -> bool {
    let mut inside = false;
    let mut j = ring.len().saturating_sub(1);
    for i in 0..ring.len() {
        let ((xi, yi), (xj, yj)) = (ring[i], ring[j]);
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

pub fn read_features(path: &Path) -> Result<Vec<Feature>> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).with_context(|| {
        format!("reading {} (run data/<city>/fetch.sh, which needs osmium)", path.display())
    })?)?;
    let all = v["features"].as_array().context("features array")?;
    // Features with a geometry type we don't use (lines) are skipped. Every other feature must
    // have an id; the extract is made with `osmium export --add-unique-id=type_id`.
    let mut out = Vec::new();
    for f in all {
        let Some(geom) = geometry(&f["geometry"]) else { continue };
        let id = f["id"].as_str().with_context(|| {
            format!("{}: feature without an id (export with --add-unique-id=type_id)", path.display())
        })?;
        out.push(Feature {
            id: id.to_string(),
            tags: f["properties"].as_object().cloned().unwrap_or_default(),
            geom,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Geom {
        Geom::Polygons(vec![vec![(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0), (0.0, 0.0)]])
    }

    #[test]
    fn point_in_polygon() {
        let g = square();
        assert!(g.contains(2.0, 2.0));
        assert!(!g.contains(5.0, 2.0));
        assert!(!g.contains(2.0, -1.0));
        assert!(!Geom::Point(1.0, 1.0).contains(1.0, 1.0));
    }

    #[test]
    fn area_is_in_square_metres() {
        // 0.001 degrees of latitude is about 110.5 m; of longitude at 42N about 82.7 m
        let g = Geom::Polygons(vec![vec![(-83.0, 42.0), (-82.999, 42.0), (-82.999, 42.001), (-83.0, 42.001), (-83.0, 42.0)]]);
        let a = g.area_m2();
        assert!((a - 110.54 * 82.73).abs() < 80.0, "{a}");
        assert_eq!(Geom::Point(0.0, 0.0).area_m2(), 0.0);
    }

    #[test]
    fn concave_polygon_excludes_the_notch() {
        // a "C" shape: the notch between x=2..4, y=1..3 is outside
        let c = Geom::Polygons(vec![vec![
            (0.0, 0.0), (4.0, 0.0), (4.0, 1.0), (2.0, 1.0), (2.0, 3.0), (4.0, 3.0), (4.0, 4.0), (0.0, 4.0), (0.0, 0.0),
        ]]);
        assert!(c.contains(1.0, 2.0));
        assert!(!c.contains(3.0, 2.0));
        assert!(c.contains(3.0, 0.5));
    }

    #[test]
    fn a_feature_without_an_id_is_an_error_not_a_silent_drop() {
        let v = serde_json::json!({"features": [
            {"properties": {"shop": "mall"}, "geometry": {"type": "Point", "coordinates": [1.0, 2.0]}}
        ]});
        let dir = std::env::temp_dir().join(format!("psim_osm_noid_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("f.geojson");
        std::fs::write(&path, v.to_string()).unwrap();
        let err = read_features(&path).unwrap_err().to_string();
        assert!(err.contains("add-unique-id"), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn parses_points_and_multipolygons_and_cleans_names() {
        let v = serde_json::json!({"features": [
            {"id": "n1", "properties": {"name": "A\u{a0}B", "shop": "mall"}, "geometry": {"type": "Point", "coordinates": [1.0, 2.0]}},
            {"id": "w2", "properties": {"amenity": "hospital"}, "geometry": {"type": "MultiPolygon", "coordinates": [[[[0,0],[2,0],[2,2],[0,2],[0,0]]]]}},
            {"id": "x3", "properties": {}, "geometry": {"type": "LineString", "coordinates": [[0,0],[1,1]]}}
        ]});
        let dir = std::env::temp_dir().join(format!("psim_osm_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("f.geojson");
        std::fs::write(&path, v.to_string()).unwrap();
        let fs = read_features(&path).unwrap();
        assert_eq!(fs.len(), 2, "line strings are skipped");
        assert_eq!(fs[0].name().as_deref(), Some("A B"));
        assert_eq!(fs[1].geom.centroid(), (1.0, 1.0));
        assert!(fs[1].geom.is_polygon());
        let _ = std::fs::remove_dir_all(dir);
    }
}
