use common::{input::Keycode, Game};
use components::{Camera, EnvironmentMap, GltfModel, Transform};
use glam::Vec2;

pub mod debug_ui;

pub fn load_scene(game: &mut Game) {
    let _span = tracy_client::span!("System: load_scene");

    if matches!(game.scene_state, common::SceneState::Unloaded) {
        let scene_path = game.scene_path.clone();
        let asset = game.asset_cache.load(&game.scene_path, move || {
            let bytes = common::assets::load_file(&format!("scenes/{scene_path}.json")).unwrap();

            serde_scene::load_scene(&bytes)
        });

        let Some(asset) = asset.try_get() else { return };

        for entity in &asset.objects {
            game.world.spawn((
                GltfModel { path: format!("models/{}", entity.file) },
                Transform { local: entity.transform },
            ));
        }
        game.world.spawn((EnvironmentMap { path: String::from("environments/HDRI.ktx2") },));

        game.scene_state = common::SceneState::Loading;
    }
}

pub fn apply_mouse_input(game: &mut Game) {
    let _span = tracy_client::span!("System: apply_mouse_input");

    for (_, camera) in game.world.query_mut::<&mut Camera>() {
        camera.pan -= game.input.mouse_delta.x * 0.001;
        camera.tilt -= game.input.mouse_delta.y * 0.001;
    }

    game.input.mouse_delta = Vec2::ZERO;
}

pub fn move_camera(game: &mut Game) {
    let _span = tracy_client::span!("System: move_camera");

    for (_, camera) in game.world.query_mut::<&mut Camera>() {
        let y = game.input.buttons.contains(&Keycode::W) as i32
            - game.input.buttons.contains(&Keycode::S) as i32;

        let x = game.input.buttons.contains(&Keycode::D) as i32
            - game.input.buttons.contains(&Keycode::A) as i32;

        let z = game.input.buttons.contains(&Keycode::Q) as i32
            - game.input.buttons.contains(&Keycode::Z) as i32;

        camera.location += glam::Vec3::new(x as f32, y as f32, z as f32) * 0.1;
    }
}
