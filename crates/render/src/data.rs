use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};

#[derive(Zeroable, Pod, Copy, Clone, Debug)]
#[repr(C)]
pub struct UniformData {
    pub proj: Mat4,
    pub inv_view_projection_no_transform: Mat4,
    pub frame_number: u32,
    pub light_count: u32,
    pub _padding: [u32; 2],
}

#[derive(Zeroable, Pod, Copy, Clone, Debug)]
#[repr(C)]
pub struct ObjectData {
    pub model_matrix: Mat4,
    pub model_view_matrix: Mat4,
    pub diffuse_texture: u32,
    pub metallic_roughnes_texture: u32,
    pub _padding: [u32; 2],
}

#[derive(Zeroable, Pod, Copy, Clone, Debug)]
#[repr(C)]
pub struct VertexData {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

#[derive(Zeroable, Pod, Copy, Clone, Debug)]
#[repr(C)]
pub struct LightData {
    pub position: Vec3,
    pub radius: f32,
    pub color: Vec3,
    pub _padding: f32,
}
