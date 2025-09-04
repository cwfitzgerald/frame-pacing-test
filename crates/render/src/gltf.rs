use std::sync::Arc;

use common::assets::{Asset, AssetCache};
use glam::Mat4;

use crate::data;

struct StubTextureGenerator {
    base_index: usize,
    textures: Vec<Asset<crate::TextureDescriptor>>,
}

impl StubTextureGenerator {
    fn from_base_index(base_index: usize) -> Self {
        Self { base_index, textures: Vec::new() }
    }

    fn generate(
        &mut self,
        asset_cache: &AssetCache,
        texture_info: Option<gltf::texture::Info>,
        color: [f32; 4],
    ) -> usize {
        match texture_info {
            Some(tex_info) => tex_info.texture().index(),
            None => {
                let color_int = (glam::Vec4::from_array(color) * 255.0).as_uvec4();
                let color_rgba8 =
                    [color_int.x as u8, color_int.y as u8, color_int.z as u8, color_int.w as u8];

                let name = format!(
                    "stub texture #{:02X}{:02X}{:02X}{:02X}",
                    color_rgba8[0], color_rgba8[1], color_rgba8[2], color_rgba8[3]
                );

                let texture = crate::TextureDescriptor {
                    size: glam::UVec2::new(1, 1),
                    mipmaps: 1,
                    depth: 1,
                    dimension: crate::TextureDimension::D2,
                    format: common::AssetTextureFormat::Rgba8,
                    path: name.clone(),
                    data: color_rgba8.to_vec(),
                };

                let asset = asset_cache.load_blocking(&name, move || texture);

                let tex_index = self.base_index + self.textures.len();
                self.textures.push(asset);

                tex_index
            }
        }
    }
}

pub(crate) struct GltfObject {
    pub transform: glam::Mat4,
    pub mesh_index: usize,
    pub diffuse_texture_index: usize,
    pub metallic_roughness_texture_index: usize,
}

pub(crate) struct GltfModelData {
    pub meshes: Vec<Arc<crate::MeshDescriptor>>,
    pub textures: Vec<Asset<crate::TextureDescriptor>>,
    pub objects: Vec<GltfObject>,
}

impl GltfModelData {
    pub fn new(asset_cache: AssetCache, path: String) -> Self {
        let _span = tracy_client::span!("Loading GLTF");

        let data = common::assets::load_file(&path).unwrap();

        let gltf = gltf::Gltf::from_slice(&data).unwrap();

        let mut future_textures = Vec::new();

        for texture in gltf.textures() {
            let gltf::image::Source::Uri { uri: buffer_url, .. } = texture.source().source() else {
                panic!("Texture source is not a URI");
            };
            let owned_buffer_url = format!("models/{buffer_url}");

            let asset =
                asset_cache.load(buffer_url, move || crate::ktx2::load_texture(&owned_buffer_url));

            future_textures.push(asset);
        }

        // Load buffers

        let buffers = gltf.buffers().collect::<Vec<_>>();

        assert_eq!(buffers.len(), 1);

        let gltf::buffer::Source::Uri(buffer_url) = buffers[0].source() else {
            panic!("Buffer source is not a URI");
        };

        let buffer = common::assets::load_file(&format!("models/{buffer_url}")).unwrap();

        let mut meshes = Vec::new();

        let mut mesh_primitive_start = Vec::new();

        for mesh in gltf.meshes() {
            mesh_primitive_start.push(meshes.len());

            for primitive in mesh.primitives() {
                let reader = primitive.reader(|b| {
                    assert_eq!(b.index(), 0);
                    Some(&buffer[..b.length()])
                });

                let positions = reader.read_positions().unwrap();
                let mut normals = reader.read_normals().unwrap();
                let mut tex_coords = reader.read_tex_coords(0).unwrap().into_f32();
                let index_data = reader.read_indices().unwrap().into_u32().collect::<Vec<_>>();

                let mut vertex_data = Vec::with_capacity(positions.len());
                for position in positions {
                    let normal = normals.next().unwrap();
                    let uv = tex_coords.next().unwrap();

                    vertex_data.push(data::VertexData { position, normal, uv });
                }

                meshes.push(Arc::new(crate::MeshDescriptor { vertex_data, index_data }));
            }
        }

        let mut stub_gen = StubTextureGenerator::from_base_index(future_textures.len());
        let mut objects = Vec::new();

        for node in gltf.nodes() {
            let mesh_primitive_start = mesh_primitive_start[node.mesh().unwrap().index()];

            for primitive in node.mesh().unwrap().primitives() {
                let mesh_index = mesh_primitive_start + primitive.index();

                let material = primitive.material();
                let pbr_metallic_roughness = material.pbr_metallic_roughness();

                let diffuse_texture_index = stub_gen.generate(
                    &asset_cache,
                    pbr_metallic_roughness.base_color_texture(),
                    pbr_metallic_roughness.base_color_factor(),
                );
                let metallic_roughness_texture_index = stub_gen.generate(
                    &asset_cache,
                    pbr_metallic_roughness.metallic_roughness_texture(),
                    [
                        0.0,
                        pbr_metallic_roughness.roughness_factor(),
                        pbr_metallic_roughness.metallic_factor(),
                        0.0,
                    ],
                );

                let transform = common::coords::GLTF_TO_WORLD_MATRIX
                    * Mat4::from_cols_array_2d(&node.transform().matrix());

                objects.push(GltfObject {
                    transform,
                    mesh_index,
                    diffuse_texture_index,
                    metallic_roughness_texture_index,
                });
            }
        }

        let mut textures: Vec<_> =
            future_textures.into_iter().map(|texture| texture.wait()).collect();
        textures.extend(stub_gen.textures);

        Self { meshes, textures, objects }
    }
}
