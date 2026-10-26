//! Neighborhood outlines for the map.
//!
//! We have no official boundaries, only the housing points in each neighborhood.
//! The outline is the convex hull of those points, pushed outward a little so a
//! neighborhood with few points still shows as an area. It is an approximation,
//! not a boundary, and the site should describe it that way.

const PAD_DEGREES: f64 = 0.0015; // about 170 m
const MIN_RADIUS_DEGREES: f64 = 0.003; // about 330 m, for 1-2 points or a straight line

type Point = (f64, f64); // (lon, lat)

fn cross(o: Point, a: Point, b: Point) -> f64 {
    (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
}

/// Convex hull, counter-clockwise, no repeated first point (Andrew's monotone chain).
fn convex_hull(points: &[Point]) -> Vec<Point> {
    let mut pts: Vec<Point> = points.to_vec();
    pts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let mut hull: Vec<Point> = Vec::new();
    for pass in 0..2 {
        let start = hull.len();
        let iter: Box<dyn Iterator<Item = &Point>> =
            if pass == 0 { Box::new(pts.iter()) } else { Box::new(pts.iter().rev()) };
        for &p in iter {
            while hull.len() >= start + 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
                hull.pop();
            }
            hull.push(p);
        }
        hull.pop();
    }
    hull
}

/// A closed ring (first point repeated at the end), counter-clockwise.
pub fn outline(points: &[Point]) -> Vec<[f64; 2]> {
    let hull = convex_hull(points);
    let n = points.len().max(1) as f64;
    let centroid = (
        points.iter().map(|p| p.0).sum::<f64>() / n,
        points.iter().map(|p| p.1).sum::<f64>() / n,
    );
    let cos_lat = centroid.1.to_radians().cos().max(0.1);

    let mut ring: Vec<[f64; 2]> = if hull.len() < 3 {
        // one point, two points, or all in a line: a small circle around the centre
        (0..12)
            .map(|i| {
                let a = std::f64::consts::TAU * f64::from(i) / 12.0;
                [centroid.0 + MIN_RADIUS_DEGREES / cos_lat * a.cos(), centroid.1 + MIN_RADIUS_DEGREES * a.sin()]
            })
            .collect()
    } else {
        hull.iter()
            .map(|&(x, y)| {
                let (dx, dy) = ((x - centroid.0) * cos_lat, y - centroid.1);
                let len = (dx * dx + dy * dy).sqrt().max(1e-12);
                [x + dx / len * PAD_DEGREES / cos_lat, y + dy / len * PAD_DEGREES]
            })
            .collect()
    };
    for p in &mut ring {
        p[0] = (p[0] * 1e6).round() / 1e6;
        p[1] = (p[1] * 1e6).round() / 1e6;
    }
    ring.push(ring[0]);
    ring
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(ring: &[[f64; 2]]) -> f64 {
        ring.windows(2).map(|w| w[0][0] * w[1][1] - w[1][0] * w[0][1]).sum::<f64>() / 2.0
    }

    #[test]
    fn hull_drops_interior_points_and_is_counter_clockwise() {
        let pts = [(0.0, 0.0), (2.0, 0.0), (2.0, 2.0), (0.0, 2.0), (1.0, 1.0)];
        let hull = convex_hull(&pts);
        assert_eq!(hull.len(), 4);
        let ring: Vec<[f64; 2]> = hull.iter().map(|p| [p.0, p.1]).chain([[hull[0].0, hull[0].1]]).collect();
        assert!(area(&ring) > 0.0);
    }

    #[test]
    fn outline_is_a_closed_counter_clockwise_ring_that_contains_the_points() {
        let pts = [(-83.75, 42.27), (-83.73, 42.27), (-83.74, 42.29)];
        let ring = outline(&pts);
        assert_eq!(ring.first(), ring.last());
        assert!(area(&ring) > 0.0);
        let (min_x, max_x) = ring.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p[0]), b.max(p[0])));
        assert!(min_x < -83.75 && max_x > -83.73, "padded outward");
    }

    #[test]
    fn one_or_two_points_still_give_an_area() {
        for pts in [vec![(-83.74, 42.28)], vec![(-83.74, 42.28), (-83.73, 42.28)]] {
            let ring = outline(&pts);
            assert_eq!(ring.len(), 13);
            assert!(area(&ring) > 0.0);
        }
    }

    #[test]
    fn collinear_points_fall_back_to_a_circle() {
        let ring = outline(&[(-83.75, 42.27), (-83.74, 42.27), (-83.73, 42.27)]);
        assert_eq!(ring.len(), 13);
        assert!(area(&ring) > 0.0);
    }
}
