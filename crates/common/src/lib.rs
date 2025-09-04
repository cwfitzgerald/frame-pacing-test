use std::time::Instant;

pub mod assets;
pub mod coords;
pub mod input;
mod panic;
pub mod qpc;
mod threadpool;

pub fn initialize() {
    panic::register_panic_hook();
    threadpool::init_threadpool();
}

pub enum AssetTextureFormat {
    Rgba8,
    Rgb9e5,
    BC1,
    BC5,
    BC6,
    BC7,
}

pub enum SceneState {
    Unloaded,
    Loading,
    Loaded,
}

pub struct EguiState {
    pub ctx: egui::Context,
    /// The most recent output from the egui ui rendering.
    pub output: egui::FullOutput,
}

impl EguiState {
    pub fn new() -> Self {
        let ctx = egui::Context::default();
        let output = egui::FullOutput::default();

        Self { ctx, output }
    }
}

pub struct Game {
    pub world: hecs::World,
    pub egui: EguiState,
    pub input: input::InputManager,
    pub start_time: Instant,
    pub scene_path: String,
    pub scene_state: SceneState,
    pub asset_cache: assets::AssetCache,
}

impl Game {
    pub fn new(scene: String) -> Self {
        let world = hecs::World::new();
        let egui = EguiState::new();
        let input = input::InputManager::new();
        let start_time = Instant::now();
        let asset_cache = assets::AssetCache::new();
        let scene_state = SceneState::Unloaded;

        Self { world, egui, input, start_time, asset_cache, scene_path: scene, scene_state }
    }
}
