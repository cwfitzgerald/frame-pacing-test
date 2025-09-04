use arrayvec::ArrayVec;
use glam::UVec2;
use windows::Win32::Graphics::Direct3D12::{ID3D12CommandQueue, ID3D12Device, ID3D12Resource};

use crate::renderer::FRAMES_IN_FLIGHT;

mod dcomp;
mod dxgi;

pub use dcomp::DCompSwapchain;
pub use dxgi::DXGISwapchain;

pub trait Swapchain {
    fn resize(&mut self, d3d12_device: &ID3D12Device, size: UVec2);
    fn frame_index(&self) -> u64;
    fn get_buffers(&self) -> ArrayVec<ID3D12Resource, { FRAMES_IN_FLIGHT }>;
    fn acquire(&mut self);
    fn present(&mut self, d3d12_queue: &ID3D12CommandQueue);
}
