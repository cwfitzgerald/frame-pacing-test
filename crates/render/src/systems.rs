use std::mem;

use bytemuck::Zeroable;
use common::{assets::Asset, Game};
use components::Transform;
use glam::{Mat3, Mat4, Vec3, Vec4};

use crate::{gltf, Renderer, TexturePointer};

struct LoadedModel {
    objects: Vec<crate::LoadedObjects>,
}

struct LoadedEnvironmentMap {
    texture_data: Asset<TexturePointer>,
}

impl Renderer {
    pub fn system_load_resources(&mut self, game: &mut Game) {
        let _span = tracy_client::span!("System: Renderer::load_resources");

        let mut cmd = hecs::CommandBuffer::new();

        for (entity, gltf_model) in
            game.world.query_mut::<&components::GltfModel>().without::<&LoadedModel>()
        {
            let clone_gltf_model = gltf_model.path.clone();
            let asset_cache = game.asset_cache.clone();
            let asset = game.asset_cache.load(&gltf_model.path, move || {
                gltf::GltfModelData::new(asset_cache, clone_gltf_model)
            });

            let Some(asset) = asset.try_get() else { continue };

            let mut texture_data = Vec::with_capacity(asset.textures.len());
            for texture in &asset.textures {
                let tex_asset = game
                    .asset_cache
                    .load_blocking(&texture.path, || self.upload_image(texture.clone()));

                texture_data.push(tex_asset);
            }

            let mesh_data: Vec<_> =
                asset.meshes.iter().cloned().map(|mesh| self.upload_mesh(mesh)).collect();

            let objects: Vec<_> = asset
                .objects
                .iter()
                .map(|object| crate::LoadedObjects {
                    transform: object.transform,
                    mesh_data: mesh_data[object.mesh_index],
                    texture_data: crate::TextureDataPointers {
                        diffuse: texture_data[object.diffuse_texture_index].clone(),
                        metallic_roughness: texture_data[object.metallic_roughness_texture_index]
                            .clone(),
                    },
                })
                .collect();

            cmd.insert_one(entity, LoadedModel { objects });
        }

        for (entity, environment_map) in
            game.world.query_mut::<&components::EnvironmentMap>().without::<&LoadedEnvironmentMap>()
        {
            let path = environment_map.path.clone();

            let asset = game
                .asset_cache
                .load(&environment_map.path, move || crate::ktx2::load_texture(&path));

            let Some(asset) = asset.try_get() else { continue };

            let texture_data = game
                .asset_cache
                .load_blocking(&environment_map.path, || self.upload_image(asset.clone()));

            cmd.insert_one(entity, LoadedEnvironmentMap { texture_data });
        }

        cmd.run_on(&mut game.world);
    }

    pub fn system_render(&mut self, game: &mut Game) {
        let _span = tracy_client::span!("System: Renderer::render");

        let (_, camera) = game.world.query_mut::<&components::Camera>().into_iter().next().unwrap();

        let look_vector =
            Mat3::from_euler(glam::EulerRot::ZXY, camera.pan, camera.tilt, 0.0) * Vec3::Y;

        let view_matrix = look_at_blendy(camera.location, camera.location + look_vector, Vec3::Z);
        let view_matrix_no_transform = look_at_blendy(Vec3::ZERO, look_vector, Vec3::Z);

        let mut objects = Vec::new();
        for (_, (loaded_model, transform)) in game.world.query_mut::<(&LoadedModel, &Transform)>() {
            for object in &loaded_model.objects {
                let mut object = object.clone();
                object.transform = transform.local * object.transform;
                objects.push(object.clone());
            }
        }

        let environment = game
            .world
            .query_mut::<&LoadedEnvironmentMap>()
            .into_iter()
            .next()
            .map(|(_, env)| &*env.texture_data);

        let lights = [crate::LightData {
            position: Vec3::new(0.0, 10.0, 10.0),
            radius: 1000.0,
            color: Vec3::new(10.0, 10.0, 10.0),
            ..Zeroable::zeroed()
        }];

        let egui_output = mem::take(&mut game.egui.output);
        let egui_input = process_egui_output(&game.egui.ctx, egui_output);

        self.render(
            environment,
            view_matrix,
            view_matrix_no_transform,
            &objects,
            &lights,
            egui_input,
        );
    }
}

fn process_egui_output(
    ctx: &egui::Context,
    output: egui::FullOutput,
) -> crate::renderer::egui::EguiRendererInput {
    let triangles = ctx.tessellate(output.shapes, output.pixels_per_point);

    crate::renderer::egui::EguiRendererInput {
        textures_delta: output.textures_delta,
        primitives: triangles,
    }
}

fn look_at_blendy(eye: Vec3, target: Vec3, up: Vec3) -> Mat4 {
    let forward = (target - eye).normalize();
    let right = forward.cross(up).normalize();
    let up = right.cross(forward);

    let orientation =
        Mat4::from_cols(right.extend(0.0), forward.extend(0.0), up.extend(0.0), Vec4::W)
            .transpose();

    let translation = Mat4::from_translation(-eye);

    orientation * translation
}
