use geojson::{GeoJson, Value};
use crate::globe::{lat_lng_to_vec3, Vec3};

pub fn build_border_lines(geojson_str: &str, radius: f32) -> Vec<(Vec3, Vec3)> {
    let geojson: GeoJson = geojson_str.parse().expect("invalid geojson");
    let mut segments = Vec::new();

    let GeoJson::FeatureCollection(fc) = geojson else { return segments };
    for feature in fc.features {

        let rings: Vec<Vec<Vec<f64>>> = match feature.geometry.map(|g| g.value) {
            Some(Value::Polygon(rings)) => rings,
            Some(Value::MultiPolygon(polys)) => polys.into_iter().flatten().collect(),
            _ => continue,
        };

        for ring in rings {
            let raised = radius * 1.002;
            let points: Vec<Vec3> = ring.into_iter()
                .map(|c: Vec<f64>| lat_lng_to_vec3(c[1], c[0], raised))
                .collect();
            for pair in points.windows(2) {
                segments.push((pair[0], pair[1]));
            }
        }
    }
    segments
}
