use common::{assets::Asset, AssetTextureFormat};

pub use crate::{
    data::{LightData, VertexData},
    renderer::Renderer,
};

mod data;
mod gltf;
mod ktx2;
mod query;
mod renderer;
mod resource;
mod swapchain;
mod systems;
mod util;

pub struct MeshDescriptor {
    pub vertex_data: Vec<VertexData>,
    pub index_data: Vec<u32>,
}

pub enum TextureDimension {
    D2,
    Cube,
    D3,
}

pub struct TextureDescriptor {
    pub size: glam::UVec2,
    pub mipmaps: u16,
    pub depth: u16,
    pub dimension: TextureDimension,
    pub format: AssetTextureFormat,
    pub path: String,
    pub data: Vec<u8>,
}

#[derive(Clone, Copy)]
pub struct MeshDataPointers {
    pub vertex_offset: u64,
    pub index_offset: u64,
    pub index_count: u32,
}

#[derive(Debug, Copy, Clone)]
pub struct TexturePointer(u32);

#[derive(Clone)]
pub struct TextureDataPointers {
    pub diffuse: Asset<TexturePointer>,
    pub metallic_roughness: Asset<TexturePointer>,
}

#[derive(Clone)]
struct LoadedObjects {
    transform: glam::Mat4,
    mesh_data: crate::MeshDataPointers,
    texture_data: crate::TextureDataPointers,
}
