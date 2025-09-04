//! Coorindate spaces in the game:
//!
//! Blender/World:
//! +X right
//! +Y forward
//! +Z up
//!
//! Conventionally track goes "forward" at the beginning of the route.
//!
//! GLTF/Clip Space:
//! +X right
//! -Z forward
//! +Y up

use glam::{Mat4, Vec4};

/// Matrix to convert from GLTF space to world space.
///
/// X -> X
/// Y -> Z
/// Z -> -Y
/// W -> W
pub const GLTF_TO_WORLD_MATRIX: Mat4 = Mat4::from_cols(
    Vec4::new(1.0, 0.0, 0.0, 0.0),
    Vec4::new(0.0, 0.0, 1.0, 0.0),
    Vec4::new(0.0, -1.0, 0.0, 0.0),
    Vec4::new(0.0, 0.0, 0.0, 1.0),
);

/// Matrix to convert from world space to clip space.
///
/// X -> X
/// Y -> -Z
/// Z -> Y
/// W -> W

pub const WORLD_TO_CLIP_MATRIX: Mat4 = Mat4::from_cols(
    Vec4::new(1.0, 0.0, 0.0, 0.0),
    Vec4::new(0.0, 0.0, -1.0, 0.0),
    Vec4::new(0.0, 1.0, 0.0, 0.0),
    Vec4::new(0.0, 0.0, 0.0, 1.0),
);

#[test]
fn invert() {
    assert_eq!(GLTF_TO_WORLD_MATRIX.inverse(), WORLD_TO_CLIP_MATRIX);
}
