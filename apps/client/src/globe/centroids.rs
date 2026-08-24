use std::collections::HashMap;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct Centroid { pub name: String, pub lat: f64, pub lng: f64 }

pub struct CentroidTable(HashMap<String, Centroid>);

impl CentroidTable {
    pub fn load() -> Self {
        const RAW: &str = include_str!("../../assets/globe/country_centroids.json");
        let map: HashMap<String, Centroid> = serde_json::from_str(RAW)
            .expect("bundled centroid file is malformed");
        Self(map)
    }

    pub fn get(&self, iso_code: &str) -> Option<&Centroid> {
        self.0
            .get(iso_code)
            .or_else(|| self.0.get(&iso_code.to_uppercase()))
            .or_else(|| self.0.get(&iso_code.to_lowercase()))
    }
}
