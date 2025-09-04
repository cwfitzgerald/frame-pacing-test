use glam::{Mat4, Vec3};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct Object {
    pub name: String,
    pub file: String,
    pub transform: Mat4,
}
#[derive(Serialize, Deserialize, Debug)]
pub struct BeizerPoint {
    pub control_point: Vec3,
    pub handle_left: Vec3,
    pub handle_right: Vec3,
    pub tilt: f32,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Curve {
    pub transform: Mat4,
    pub points: Vec<BeizerPoint>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Scene {
    pub objects: Vec<Object>,
    pub curves: Vec<Curve>,
}

pub fn load_scene(bytes: &[u8]) -> Scene {
    serde_json::from_slice(bytes).unwrap()
}
