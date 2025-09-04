use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub struct GltfModel {
    pub path: String,
}

#[derive(Deserialize, Serialize)]
pub struct Transform {
    pub local: glam::Mat4,
}

#[derive(Deserialize, Serialize)]
pub struct Camera {
    pub location: glam::Vec3,
    pub pan: f32,
    pub tilt: f32,
}

#[derive(Deserialize, Serialize)]
pub struct EnvironmentMap {
    pub path: String,
}
