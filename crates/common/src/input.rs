use std::collections::HashSet;

use glam::Vec2;

pub type Keycode = sdl2::keyboard::Keycode;

pub struct InputManager {
    pub mouse_delta: Vec2,
    pub buttons: HashSet<Keycode>,
}

impl InputManager {
    pub fn new() -> Self {
        Self { mouse_delta: Vec2::ZERO, buttons: HashSet::new() }
    }
}

impl Default for InputManager {
    fn default() -> Self {
        Self::new()
    }
}
