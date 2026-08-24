pub mod centroids;
pub mod camera_tween;
pub mod borders;
pub mod renderer;

pub use centroids::{Centroid, CentroidTable};

#[derive(Clone, Copy, Debug)]
pub struct Vec3 { pub x: f32, pub y: f32, pub z: f32 }

impl Vec3 {
    pub fn new(x: f32, y: f32, z: f32) -> Self { Self { x, y, z } }
}

impl std::ops::Add for Vec3 {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }
}

impl std::ops::Sub for Vec3 {
    type Output = Self;
    fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }
}

impl std::ops::Mul<f32> for Vec3 {
    type Output = Self;
    fn mul(self, scalar: f32) -> Self {
        Self::new(self.x * scalar, self.y * scalar, self.z * scalar)
    }
}

pub fn lat_lng_to_vec3(lat: f64, lng: f64, radius: f32) -> Vec3 {
    let phi = (90.0 - lat).to_radians();
    let theta = (lng + 180.0).to_radians();
    Vec3::new(
        (-radius as f64 * phi.sin() * theta.cos()) as f32,
        (radius as f64 * phi.cos()) as f32,
        (radius as f64 * phi.sin() * theta.sin()) as f32,
    )
}

pub fn nearest_country(click_lat: f64, click_lng: f64, centroids: &CentroidTable, isos: &[String]) -> Option<String> {
    isos.iter()
        .filter_map(|iso| centroids.get(iso).map(|c| (iso.clone(), haversine_km(click_lat, click_lng, c.lat, c.lng))))
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
        .map(|(iso, _)| iso)
}

fn haversine_km(lat1: f64, lng1: f64, lat2: f64, lng2: f64) -> f64 {
    let r = 6371.0;
    let d_lat = (lat2 - lat1).to_radians();
    let d_lng = (lng2 - lng1).to_radians();
    let a = (d_lat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (d_lng / 2.0).sin().powi(2);
    2.0 * r * a.sqrt().asin()
}
