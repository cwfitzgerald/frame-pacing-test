use std::time::Duration;

use anyhow::Context;
use arrayvec::ArrayVec;
use glam::UVec2;
use windows::{
    core::Interface,
    Win32::{
        Foundation::{HANDLE, HWND, WAIT_OBJECT_0},
        Graphics::{
            Direct3D12::*,
            DirectComposition::{
                DCompositionBoostCompositorClock, DCompositionGetFrameId,
                DCompositionGetStatistics, DCompositionGetTargetStatistics,
                COMPOSITION_FRAME_ID_COMPLETED, COMPOSITION_FRAME_STATS, COMPOSITION_TARGET_ID,
                COMPOSITION_TARGET_STATS,
            },
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
    previous_frame_time: u64,
    sleeper: spin_sleep_util::Interval,
}

impl DXGISwapchain {
    pub fn new(
        dxgi_factory: &IDXGIFactory4,
        graphics_command_queue: &ID3D12CommandQueue,
        hwnd: HWND,
        size: UVec2,
        target_frame_rate: f32,
    ) -> anyhow::Result<Self> {
        unsafe { DCompositionBoostCompositorClock(true).unwrap() };

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

            Ok(Self {
                swapchain,
                waitable_object,
                buffers,
                present_index: 0,
                previous_frame_time: 0,
                sleeper: spin_sleep_util::interval(Duration::from_nanos(
                    (1_000_000_000.0 / target_frame_rate) as u64,
                )),
            })
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

        self.sleeper.tick();

        assert_eq!(wait_result, WAIT_OBJECT_0, "Failed to wait for swapchain frame latency object");

        let mut dxgi_stats = DXGI_FRAME_STATISTICS::default();
        unsafe {
            let _ = self.swapchain.GetFrameStatistics(&mut dxgi_stats);
        }

        let dxgi_diff = Duration::from_nanos(
            (dxgi_stats.SyncQPCTime as u64).saturating_sub(self.previous_frame_time) * 100,
        );
        self.previous_frame_time = dxgi_stats.SyncQPCTime as u64;

        log::info!("DXGI Frame Duration: {:?}", dxgi_diff);

        unsafe {
            let composition_frame_id =
                DCompositionGetFrameId(COMPOSITION_FRAME_ID_COMPLETED).unwrap();

            let mut frame_stats = COMPOSITION_FRAME_STATS::default();
            let mut target_ids = [COMPOSITION_TARGET_ID::default(); 8];
            let mut target_id_count = 0;

            DCompositionGetStatistics(
                composition_frame_id,
                &mut frame_stats,
                target_ids.len() as _,
                Some(target_ids.as_mut_ptr()),
                Some(&mut target_id_count),
            )
            .unwrap();

            let mut target_stats = [COMPOSITION_TARGET_STATS::default(); 8];
            for i in 0..target_id_count as usize {
                target_stats[i] =
                    DCompositionGetTargetStatistics(composition_frame_id, &target_ids[i]).unwrap();
            }

            for i in 0..target_id_count as usize {
                let stat = &target_stats[i];
                let vblank_duration = Duration::from_nanos(stat.vblankDuration * 100);

                log::info!(
                    "{i} VBlank Duration: {:?}, Outstanding Presents: {}",
                    vblank_duration,
                    stat.outstandingPresents,
                );
            }
        }
    }

    fn present(&mut self, _d3d12_queue: &ID3D12CommandQueue) {
        unsafe {
            self.swapchain.Present(1, DXGI_PRESENT(0)).unwrap();

            self.present_index += 1;
        }
    }
}
