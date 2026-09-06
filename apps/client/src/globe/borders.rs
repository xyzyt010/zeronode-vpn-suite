use crate::globe::{lat_lng_to_vec3, Vec3};
use geojson::{GeoJson, Value};

/// One closed (or open) coastline ring in unit-sphere space.
pub type BorderRing = Vec<Vec3>;

/// Build globe coast rings from Natural Earth GeoJSON.
///
/// RAM / smoothness notes:
/// - Exterior rings only (lakes/holes are invisible at globe scale).
/// - Light Ramer–Douglas–Peucker so coasts stay smooth but vertex count drops
///   ~5–8× versus raw 50m data.
/// - Stored as polylines (not per-edge segments) so the painter tessellates
///   hundreds of shapes instead of hundreds of thousands.
pub fn build_border_rings(geojson_str: &str, radius: f32) -> Vec<BorderRing> {
    let geojson: GeoJson = geojson_str.parse().expect("invalid geojson");
    let GeoJson::FeatureCollection(fc) = geojson else {
        return Vec::new();
    };

    let raised = radius * 1.002;
    // ~5 km at the equator — still looks smooth at COUNTRY_FOCUS_ZOOM.
    const EPS_DEG: f64 = 0.045;
    let mut rings_out = Vec::with_capacity(300);

    for feature in fc.features {
        let polys: Vec<Vec<Vec<Vec<f64>>>> = match feature.geometry.map(|g| g.value) {
            Some(Value::Polygon(rings)) => vec![rings],
            Some(Value::MultiPolygon(polys)) => polys,
            _ => continue,
        };
        for poly in polys {
            let Some(ring) = poly.into_iter().next() else {
                continue;
            };
            if ring.len() < 4 {
                continue;
            }
            let simplified = rdp_ring(&ring, EPS_DEG);
            if simplified.len() < 4 {
                continue;
            }
            let points: Vec<Vec3> = simplified
                .into_iter()
                .map(|c| lat_lng_to_vec3(c[1], c[0], raised))
                .collect();
            rings_out.push(points);
        }
    }
    rings_out.shrink_to_fit();
    rings_out
}

/// Iterative Ramer–Douglas–Peucker on lon/lat rings. Keeps endpoints.
fn rdp_ring(ring: &[Vec<f64>], eps: f64) -> Vec<Vec<f64>> {
    let n = ring.len();
    if n < 4 {
        return ring.to_vec();
    }
    // Drop the duplicate closing vertex; we re-close after simplify.
    let end = if coords_close(&ring[0], &ring[n - 1]) {
        n - 1
    } else {
        n
    };
    if end < 3 {
        return ring.to_vec();
    }

    let mut keep = vec![false; end];
    keep[0] = true;
    keep[end - 1] = true;

    let mut stack = vec![(0usize, end - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let mut max_d = 0.0_f64;
        let mut max_i = a;
        for i in (a + 1)..b {
            let d = perp_dist_deg(&ring[i], &ring[a], &ring[b]);
            if d > max_d {
                max_d = d;
                max_i = i;
            }
        }
        if max_d > eps {
            keep[max_i] = true;
            stack.push((a, max_i));
            stack.push((max_i, b));
        }
    }

    let mut out: Vec<Vec<f64>> = (0..end)
        .filter(|&i| keep[i])
        .map(|i| ring[i].clone())
        .collect();
    if let Some(first) = out.first().cloned() {
        if !coords_close(&first, out.last().unwrap()) {
            out.push(first);
        }
    }
    out
}

fn coords_close(a: &[f64], b: &[f64]) -> bool {
    if a.len() < 2 || b.len() < 2 {
        return false;
    }
    (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
}

fn perp_dist_deg(p: &[f64], a: &[f64], b: &[f64]) -> f64 {
    if p.len() < 2 || a.len() < 2 || b.len() < 2 {
        return 0.0;
    }
    let (px, py) = (p[0], p[1]);
    let (ax, ay) = (a[0], a[1]);
    let (bx, by) = (b[0], b[1]);
    let dx = bx - ax;
    let dy = by - ay;
    let len2 = dx * dx + dy * dy;
    if len2 < 1e-18 {
        let ex = px - ax;
        let ey = py - ay;
        return (ex * ex + ey * ey).sqrt();
    }
    let t = ((px - ax) * dx + (py - ay) * dy) / len2;
    let t = t.clamp(0.0, 1.0);
    let qx = ax + t * dx;
    let qy = ay + t * dy;
    let ex = px - qx;
    let ey = py - qy;
    (ex * ex + ey * ey).sqrt()
}
