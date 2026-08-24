use three_d::{CpuMesh, Positions, Indices, vec2};
use crate::globe::lat_lng_to_vec3;

pub fn build_globe_mesh(lat_segments: u32, lng_segments: u32, radius: f32) -> CpuMesh {
    let mut positions = Vec::new();
    let mut uvs = Vec::new();

    for y in 0..=lat_segments {
        let v = y as f32 / lat_segments as f32;     // 0 = north pole, 1 = south pole
        let lat = 90.0 - v * 180.0;
        for x in 0..=lng_segments {
            let u = x as f32 / lng_segments as f32; // 0 -> 1 maps to -180 -> 180
            let lng = u * 360.0 - 180.0;
            positions.push(lat_lng_to_vec3(lat as f64, lng as f64, radius));
            uvs.push(vec2(u, v));
        }
    }

    let mut indices = Vec::new();
    for y in 0..lat_segments {
        for x in 0..lng_segments {
            let i0 = y * (lng_segments + 1) + x;
            let (i1, i2, i3) = (i0 + 1, i0 + lng_segments + 1, i0 + lng_segments + 2);
            indices.extend_from_slice(&[i0, i2, i1, i1, i2, i3]);
        }
    }

    CpuMesh {
        name: "globe".to_string(),
        positions: Positions::F32(positions),
        uvs: Some(uvs),
        indices: Indices::U32(indices),
        ..Default::default()
    }
}
