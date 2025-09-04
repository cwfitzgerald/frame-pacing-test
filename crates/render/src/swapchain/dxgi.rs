use anyhow::Context;
use arrayvec::ArrayVec;
use glam::UVec2;
use windows::{
    core::Interface,
    Win32::{
        Foundation::{HANDLE, HWND, WAIT_OBJECT_0},
        Graphics::{
            Direct3D12::*,
            Dxgi::{Common::*, *},
        },
        System::Threading::WaitForSingleObject,
    },
};

use crate::{renderer::FRAMES_IN_FLIGHT, swapchain::Swapchain};

const SWAPCHAIN_FLAGS: u32 = (DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING.0
    | DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0) as _;

pub struct DXGISwapchain {
    swapchain: IDXGISwapChain4,
    waitable_object: HANDLE,
    buffers: ArrayVec<ID3D12Resource, { FRAMES_IN_FLIGHT }>,
    present_index: u64,
}

impl DXGISwapchain {
    pub fn new(
        dxgi_factory: &IDXGIFactory4,
        graphics_command_queue: &ID3D12CommandQueue,
        hwnd: HWND,
        size: UVec2,
    ) -> anyhow::Result<Self> {
        let swapchain_desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: size.x,
            Height: size.y,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, ..Default::default() },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: FRAMES_IN_FLIGHT as _,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: SWAPCHAIN_FLAGS,
            ..Default::default()
        };

        unsafe {
            let swapchain: IDXGISwapChain4 = dxgi_factory
                .CreateSwapChainForHwnd(graphics_command_queue, hwnd, &swapchain_desc, None, None)
                .context("Failed to create swapchain for the window")?
                .cast()
                .context("Failed to cast swapchain to IDXGISwapChain4")?;

            swapchain.SetMaximumFrameLatency(2)?;

            let waitable_object = swapchain.GetFrameLatencyWaitableObject();

            dxgi_factory
                .MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER)
                .context("Failed to remove alt-enter association")?;

            let mut buffers = ArrayVec::new();
            for i in 0..FRAMES_IN_FLIGHT {
                let buffer: ID3D12Resource = swapchain
                    .GetBuffer(i as _)
                    .with_context(|| format!("Failed to get swapchain buffer {}", i))?;
                buffers.push(buffer);
            }

            Ok(Self { swapchain, waitable_object, buffers, present_index: 0 })
        }
    }
}

impl Swapchain for DXGISwapchain {
    fn resize(&mut self, _d3d12_device: &ID3D12Device, size: UVec2) {
        self.buffers.clear();

        unsafe {
            self.swapchain
                .ResizeBuffers(
                    FRAMES_IN_FLIGHT as _,
                    size.x,
                    size.y,
                    DXGI_FORMAT_R8G8B8A8_UNORM,
                    DXGI_SWAP_CHAIN_FLAG(SWAPCHAIN_FLAGS as _),
                )
                .context("Failed to resize swapchain buffers")
                .unwrap();

            for i in 0..FRAMES_IN_FLIGHT {
                let buffer: ID3D12Resource = self
                    .swapchain
                    .GetBuffer(i as _)
                    .with_context(|| format!("Failed to get swapchain buffer {}", i))
                    .unwrap();
                self.buffers.push(buffer);
            }
        }
    }

    fn frame_index(&self) -> u64 {
        self.present_index
    }

    fn get_buffers(&self) -> ArrayVec<ID3D12Resource, { FRAMES_IN_FLIGHT }> {
        self.buffers.clone()
    }

    fn acquire(&mut self) {
        let wait_result = unsafe { WaitForSingleObject(self.waitable_object, 5_000) };

        assert_eq!(wait_result, WAIT_OBJECT_0, "Failed to wait for swapchain frame latency object");
    }

    fn present(&mut self, _d3d12_queue: &ID3D12CommandQueue) {
        unsafe {
            self.swapchain.Present(1, DXGI_PRESENT(0)).unwrap();

            self.present_index += 1;
        }
    }
}
