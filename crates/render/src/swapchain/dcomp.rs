use std::{
    collections::VecDeque,
    ffi::c_void,
    mem, ptr,
    time::{Duration, Instant},
};

use anyhow::Context;
use arrayvec::ArrayVec;
use glam::UVec2;
use windows::{
    core::Interface,
    Win32::{
        Foundation::*,
        Graphics::{
            CompositionSwapchain::*,
            Direct3D::*,
            Direct3D11::*,
            Direct3D12::*,
            DirectComposition::*,
            Dxgi::{Common::*, *},
        },
        System::{Threading::*, WindowsProgramming::QueryInterruptTime},
    },
};

use crate::{query, swapchain::Swapchain, util::SmartNtHandle};
const BUFFER_COUNT: usize = 3;

struct InteropFence {
    fence12: ID3D12Fence,
    fence11: ID3D11Fence,
}

struct InteropTexture {
    texture12: ID3D12Resource,
    texture11: ID3D11Texture2D,
}

struct InteropState {
    d12_11_fence: InteropFence,
    d11_12_fence: InteropFence,
    textures: ArrayVec<InteropTexture, 2>,
}

impl InteropState {
    fn new(
        d3d12_device: &ID3D12Device,
        d3d11_device: &ID3D11Device5,
        size: UVec2,
    ) -> anyhow::Result<Self> {
        let d12_11_fence = Self::create_shared_fence(d3d12_device, d3d11_device)?;
        let d11_12_fence = Self::create_shared_fence(d3d12_device, d3d11_device)?;

        let mut textures = ArrayVec::new();
        for _ in 0..2 {
            textures.push(Self::create_shared_texture(d3d12_device, d3d11_device, size)?);
        }

        Ok(Self { d12_11_fence, d11_12_fence, textures })
    }

    fn resize(&mut self, d3d12_device: &ID3D12Device, d3d11_device: &ID3D11Device5, size: UVec2) {
        self.textures.clear();
        for _ in 0..2 {
            self.textures
                .push(Self::create_shared_texture(d3d12_device, d3d11_device, size).unwrap());
        }
    }

    fn create_shared_texture(
        d3d12_device: &ID3D12Device,
        d3d11_device: &ID3D11Device5,
        size: UVec2,
    ) -> anyhow::Result<InteropTexture> {
        unsafe {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: size.x,
                Height: size.y,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_RENDER_TARGET.0 as _,
                CPUAccessFlags: 0,
                MiscFlags: (D3D11_RESOURCE_MISC_SHARED | D3D11_RESOURCE_MISC_SHARED_NTHANDLE).0
                    as _,
            };

            let mut texture11: Option<ID3D11Texture2D> = None;
            d3d11_device
                .CreateTexture2D(&desc, None, Some(&mut texture11))
                .context("Failed to create D3D11 texture")?;
            let texture11 = texture11.unwrap();

            let resource11 = texture11.cast::<IDXGIResource1>().unwrap();
            let texture_handle = SmartNtHandle::from_raw(
                resource11
                    .CreateSharedHandle(
                        None,
                        DXGI_SHARED_RESOURCE_READ.0 | DXGI_SHARED_RESOURCE_WRITE.0,
                        None,
                    )
                    .context("Failed to create a shared handle")?,
            );

            let mut texture12: Option<ID3D12Resource> = None;
            d3d12_device
                .OpenSharedHandle(*texture_handle, &mut texture12)
                .context("Failed to open shared handle")?;
            let texture12 = texture12.unwrap();

            Ok(InteropTexture { texture12, texture11 })
        }
    }

    fn create_shared_fence(
        d3d12_device: &ID3D12Device,
        d3d11_device: &ID3D11Device5,
    ) -> anyhow::Result<InteropFence> {
        unsafe {
            let mut fence11: Option<ID3D11Fence> = None;
            d3d11_device
                .CreateFence(0, D3D11_FENCE_FLAG_SHARED, &mut fence11)
                .context("Failed to create D3D11 fence")?;
            let fence11 = fence11.unwrap();
            let fence_handle = SmartNtHandle::from_raw(
                fence11
                    .CreateSharedHandle(None, GENERIC_ALL.0, None)
                    .context("Failed to create shared fence handle")?,
            );
            let mut fence12: Option<ID3D12Fence> = None;
            d3d12_device
                .OpenSharedHandle(*fence_handle, &mut fence12)
                .context("Failed to open shared fence handle")?;
            let fence12 = fence12.unwrap();

            Ok(InteropFence { fence12, fence11 })
        }
    }
}

struct Query {
    start_query: ID3D11Query,
    end_query: ID3D11Query,
    tracy_query: Option<tracy_client::GpuSpan>,
}

impl Query {
    fn new(d3d11_device: &ID3D11Device, tracy_context: &tracy_client::GpuContext) -> Self {
        unsafe {
            let mut start_query: Option<ID3D11Query> = None;
            d3d11_device
                .CreateQuery(
                    &D3D11_QUERY_DESC { Query: D3D11_QUERY_TIMESTAMP, MiscFlags: 0 },
                    Some(&mut start_query),
                )
                .unwrap();
            let start_query = start_query.unwrap();

            let mut end_query: Option<ID3D11Query> = None;
            d3d11_device
                .CreateQuery(
                    &D3D11_QUERY_DESC { Query: D3D11_QUERY_TIMESTAMP, MiscFlags: 0 },
                    Some(&mut end_query),
                )
                .unwrap();
            let end_query = end_query.unwrap();

            let tracy_query =
                Some(tracy_context.span_alloc("Swapchain Blit", "", file!(), line!()).unwrap());

            Self { start_query, end_query, tracy_query }
        }
    }
}

pub struct DCompSwapchain {
    d3d11_device: ID3D11Device5,
    d3d11_context: ID3D11DeviceContext4,

    _dcomp_device: IDCompositionDevice,
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,

    presentation_manager: IPresentationManager,
    presentation_surface: IPresentationSurface,
    buffers: ArrayVec<PresentationBuffer, BUFFER_COUNT>,
    supports_displayable_textures: bool,

    frame_duration_100ns: u64,

    tracy_context: tracy_client::GpuContext,
    queries: VecDeque<Query>,

    interop_state: InteropState,

    retiring_fence: ID3D11Fence,

    lost_event: HANDLE,
    reset_event: SmartNtHandle,

    present_index: u64,
    buffer_index: u64,

    start_time: Option<u64>,
    previous_recorded_time: u64,
    previous_wait_time: Instant,
}

impl DCompSwapchain {
    pub fn new(
        adapter: &IDXGIAdapter,
        d3d12_device: &ID3D12Device,
        d3d12_queue: &ID3D12CommandQueue,
        hwnd: HWND,
        size: UVec2,
        target_frame_rate: f32,
    ) -> anyhow::Result<Self> {
        unsafe {
            DCompositionBoostCompositorClock(true).unwrap();

            let mut d3d11_device = None;
            let mut d3d11_context = None;
            D3D11CreateDevice(
                adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                None,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_1]),
                D3D11_SDK_VERSION,
                Some(&mut d3d11_device),
                None,
                Some(&mut d3d11_context),
            )
            .context("Failed to create D3D11 device")?;
            let d3d11_device = d3d11_device
                .unwrap()
                .cast::<ID3D11Device5>()
                .context("Failed to cast to ID3D11Device5")?;
            let d3d11_context = d3d11_context
                .unwrap()
                .cast::<ID3D11DeviceContext4>()
                .context("Failed to cast to ID3D11DeviceContext4")?;

            let mut displayable = D3D11_FEATURE_DATA_DISPLAYABLE::default();
            d3d11_device
                .CheckFeatureSupport(
                    D3D11_FEATURE_DISPLAYABLE,
                    <*mut _>::cast(&mut displayable),
                    mem::size_of_val(&displayable) as u32,
                )
                .context("Failed to check feature support")?;

            let supports_displayable_textures = displayable.DisplayableTexture == TRUE;

            log::info!("Initialized D3D11 device");
            log::info!("- Supports Displayable Textures: {}", supports_displayable_textures);

            let interop_state = InteropState::new(&d3d12_device, &d3d11_device, size)?;

            let tracy_context = query::create_d3d11_context(d3d12_queue);

            let dxgi_device =
                d3d11_device.cast::<IDXGIDevice>().context("Failed to cast to IDXGIDevice")?;

            //
            // DComp
            //

            let dcomp_device: IDCompositionDevice =
                DCompositionCreateDevice(&dxgi_device).context("Failed to create DComp device")?;

            log::info!("Initialized DComp device");

            let target = dcomp_device
                .CreateTargetForHwnd(hwnd, TRUE)
                .context("Failed to create DComp target")?;

            let visual = dcomp_device.CreateVisual().context("Failed to create DComp visual")?;

            let surface_handle = SmartNtHandle::from_raw(
                DCompositionCreateSurfaceHandle(
                    (COMPOSITIONOBJECT_READ | COMPOSITIONOBJECT_WRITE) as u32,
                    None,
                )
                .context("Failed to create DComp surface handle")?,
            );

            let dcomp_surface = dcomp_device
                .CreateSurfaceFromHandle(*surface_handle)
                .context("Failed to create DComp surface")?;

            //
            // Composition Swapchain
            //

            let factory = create_presentation_factory(&d3d11_device)?;

            anyhow::ensure!(factory.IsPresentationSupported() == 1);

            let supports_iflip = factory.IsPresentationSupportedWithIndependentFlip() == 1;

            log::info!("Initialized Composition Swapchain");
            log::info!("- Supports iFlip: {}", supports_iflip);

            let presentation_manager = factory
                .CreatePresentationManager()
                .context("Failed to create presentation manager")?;

            let presentation_surface = presentation_manager
                .CreatePresentationSurface(*surface_handle)
                .context("Failed to create presentation surface")?;

            presentation_surface
                .SetAlphaMode(DXGI_ALPHA_MODE_IGNORE)
                .context("Failed to set alpha mode")?;
            presentation_surface
                .SetColorSpace(DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709)
                .context("Failed to set color space")?;
            presentation_surface
                .SetSourceRect(&RECT {
                    left: 0,
                    top: 0,
                    right: size.x as i32,
                    bottom: size.y as i32,
                })
                .context("Failed to set source rect")?;
            presentation_surface
                .SetTransform(&PresentationTransform {
                    M11: 1.0,
                    M12: 0.0,
                    M21: 0.0,
                    M22: 1.0,
                    M31: 0.0,
                    M32: 0.0,
                })
                .context("Failed to set transform")?;
            presentation_surface
                .SetLetterboxingMargins(0.0, 0.0, 0.0, 0.0)
                .context("Failed to set letterboxing margins")?;

            let retiring_fence = presentation_manager
                .GetPresentRetiringFence(&ID3D11Fence::IID)
                .context("Failed to get retiring event")?;
            anyhow::ensure!(!retiring_fence.is_null(), "Retiring fence is null");
            let retiring_fence = ID3D11Fence::from_raw(retiring_fence);

            let lost_event =
                presentation_manager.GetLostEvent().context("Failed to get lost event")?;

            let reset_event =
                SmartNtHandle::new_manual_reset().context("Failed to create reset event")?;

            // Make the relationships

            target.SetRoot(&visual).context("Failed to set DComp target root")?;
            visual.SetContent(&dcomp_surface).context("Failed to set DComp visual content")?;

            dcomp_device.Commit().context("Failed to commit to DComp device")?;

            // Create the textures

            let mut buffers = ArrayVec::new();
            for _ in 0..BUFFER_COUNT {
                buffers.push(create_texture(
                    &presentation_manager,
                    &d3d11_device,
                    size,
                    supports_displayable_textures,
                )?);
            }

            presentation_manager
                .EnablePresentStatisticsKind(PresentStatisticsKind_IndependentFlipFrame, 1)
                .unwrap();
            presentation_manager
                .EnablePresentStatisticsKind(PresentStatisticsKind_CompositionFrame, 1)
                .unwrap();
            presentation_manager.ForceVSyncInterrupt(1).unwrap();

            let present_index = presentation_manager.GetNextPresentId();

            let frame_duration_100ns = (10_000_000.0 / target_frame_rate).round() as u64; // 100 ns units

            Ok(Self {
                d3d11_device,
                d3d11_context,
                _dcomp_device: dcomp_device,
                _target: target,
                _visual: visual,
                supports_displayable_textures,
                presentation_manager,
                presentation_surface,
                tracy_context,
                queries: VecDeque::new(),
                buffers,
                frame_duration_100ns,
                interop_state,
                retiring_fence,
                lost_event,
                reset_event,
                present_index,
                buffer_index: 0,
                start_time: None,
                previous_recorded_time: 0,
                previous_wait_time: Instant::now(),
            })
        }
    }

    fn display_stats(&mut self) {
        unsafe {
            while let Ok(stats) = self.presentation_manager.GetNextPresentStatistics() {
                let id = stats.GetPresentId();
                let kind = stats.GetKind();

                let original_target = Duration::from_nanos(
                    (self.start_time.unwrap() + self.frame_duration_100ns * id) * 100,
                );

                #[allow(non_upper_case_globals)]
                match kind {
                    PresentStatisticsKind_CompositionFrame => {
                        let stats: ICompositionFramePresentStatistics = stats.cast().unwrap();

                        let mut display_instance_array_count = 0;
                        let mut display_instance_array_ptr = ptr::null_mut();
                        stats.GetDisplayInstanceArray(
                            &mut display_instance_array_count,
                            &mut display_instance_array_ptr,
                        );

                        let display_instance_array = std::slice::from_raw_parts(
                            display_instance_array_ptr,
                            display_instance_array_count as usize,
                        );

                        let composition_frame_id = stats.GetCompositionFrameId();

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
                            DCompositionGetTargetStatistics(
                                composition_frame_id,
                                &target_ids[i],
                                &mut target_stats[i],
                            )
                            .unwrap();
                        }

                        let previous_time = self.previous_recorded_time;
                        self.previous_recorded_time = frame_stats.targetTime;

                        let previous_time = Duration::from_nanos(previous_time * 100);
                        let target_time = Duration::from_nanos(frame_stats.targetTime * 100);

                        let diff = if target_time > original_target {
                            format!("+{:?}", target_time - original_target)
                        } else {
                            format!("-{:?}", original_target - target_time)
                        };

                        let xadapter = if display_instance_array[0].requiredCrossAdapterCopy == 1 {
                            "(cross-adapter)"
                        } else {
                            ""
                        };

                        let vblank_duration =
                            Duration::from_nanos(target_stats[0].vblankDuration * 100);

                        let comp_frequency = Duration::from_nanos(frame_stats.framePeriod * 100);

                        let no_iflip =
                            if self.supports_displayable_textures { "" } else { " (no iFlip)" };

                        log::info!(
                            "Presentation {}: CompFrame: {} {xadapter}{no_iflip}\
                             Sch: {original_target:?} Tar: {target_time:?} (diff {diff}), \
                             Delta Tar: {:?}, VBlank: {:?}, Comp Feq: {:?}",
                            id,
                            composition_frame_id,
                            target_time - previous_time,
                            vblank_duration,
                            comp_frequency,
                        );
                    }
                    PresentStatisticsKind_IndependentFlipFrame => {
                        let stats: IIndependentFlipFramePresentStatistics = stats.cast().unwrap();

                        let previous_time = self.previous_recorded_time;

                        let displayed_time = stats.GetDisplayedTime().value;
                        let present_duration = stats.GetPresentDuration().value;

                        self.previous_recorded_time = displayed_time;

                        let previous_time = Duration::from_nanos(previous_time * 100);
                        let displayed_time = Duration::from_nanos(displayed_time * 100);
                        let present_duration = Duration::from_nanos(present_duration * 100);

                        let diff = if displayed_time > original_target {
                            format!("+{:?}", displayed_time - original_target)
                        } else {
                            format!("-{:?}", original_target - displayed_time)
                        };

                        let delta = if displayed_time > previous_time {
                            format!("+{:?}", displayed_time - previous_time)
                        } else {
                            format!("-{:?}", previous_time - displayed_time)
                        };

                        log::info!(
                            "Presentation {id}: Independent Flip Frame, Sch: {original_target:?} Dis: {displayed_time:?} (diff {diff}), Delta Dis {delta}, Duration {present_duration:?}",
                        );
                    }
                    _ => {}
                }
            }
        }
    }
}

impl Swapchain for DCompSwapchain {
    fn resize(&mut self, d3d12_device: &ID3D12Device, size: UVec2) {
        let buffers = &mut self.buffers;
        buffers.clear();

        for _ in 0..BUFFER_COUNT {
            buffers.push(
                create_texture(
                    &self.presentation_manager,
                    &self.d3d11_device,
                    size,
                    self.supports_displayable_textures,
                )
                .unwrap(),
            );
        }

        self.interop_state.resize(d3d12_device, &self.d3d11_device, size);

        unsafe {
            self.presentation_surface
                .SetSourceRect(&RECT {
                    left: 0,
                    top: 0,
                    right: size.x as i32,
                    bottom: size.y as i32,
                })
                .context("Failed to set source rect")
                .unwrap();
        }
    }

    fn frame_index(&self) -> u64 {
        self.present_index
    }

    fn get_buffers(&self) -> ArrayVec<ID3D12Resource, 2> {
        self.interop_state.textures.iter().map(|texture| texture.texture12.clone()).collect()
    }

    fn acquire(&mut self) {
        if self.start_time.is_none() {
            self.start_time = Some(get_interrupt_time());
        }

        unsafe {
            let target_time =
                self.start_time.unwrap() + self.frame_duration_100ns * self.present_index; // 16.667 ms in 100 ns units

            // let duration_until_target: Duration =
            //     Duration::from_nanos(target_time.saturating_sub(now).saturating_mul(100));

            // log::info!(
            //     "Starting frame {} with buffer {} and target time {} ({duration_until_target:?} from now)",
            //     self.present_index,
            //     self.buffer_index,
            //     target_time
            // );
            // log::info!("- Now: {}", now);

            self.presentation_manager
                .SetTargetTime(SystemInterruptTime { value: target_time })
                .unwrap();

            let buffer = &mut self.buffers[self.buffer_index as usize];
            let previous_present_id = buffer.previous_present_id;

            self.wait_for_present(previous_present_id);

            let now = Instant::now();
            let previous_wait_time = self.previous_wait_time;
            self.previous_wait_time = now;

            let diff = now.duration_since(previous_wait_time);

            log::info!(
                "Waited {:?} since last acquire (should be ~{:?})",
                diff,
                Duration::from_nanos(self.frame_duration_100ns * 100)
            );

            self.display_stats();

            let buffer = &mut self.buffers[self.buffer_index as usize];
            assert_eq!(buffer.buffer.IsAvailable(), Ok(1));

            self.poll_queries();
        }
    }

    fn present(&mut self, d3d12_queue: &ID3D12CommandQueue) {
        unsafe {
            d3d12_queue
                .Signal(&self.interop_state.d12_11_fence.fence12, self.present_index)
                .unwrap();
            self.d3d11_context
                .Wait(&self.interop_state.d12_11_fence.fence11, self.present_index)
                .unwrap();

            let buffer = &mut self.buffers[self.buffer_index as usize];
            let mut query = Query::new(&self.d3d11_device, &self.tracy_context);

            let src_resource =
                &self.interop_state.textures[self.present_index as usize % 2].texture11;

            self.d3d11_context.End(&query.start_query);
            self.d3d11_context.CopyResource(&buffer.texture, src_resource);
            self.d3d11_context.End(&query.end_query);
            query.tracy_query.as_mut().unwrap().end_zone();

            self.queries.push_back(query);

            self.presentation_surface.SetBuffer(&buffer.buffer).unwrap();
            self.presentation_manager
                .SetPreferredPresentDuration(
                    SystemInterruptTime { value: self.frame_duration_100ns },
                    SystemInterruptTime { value: 0 },
                )
                .unwrap();

            self.presentation_manager.Present().unwrap();

            self.d3d11_context
                .Signal(&self.interop_state.d11_12_fence.fence11, self.present_index)
                .unwrap();
            d3d12_queue
                .Wait(
                    &self.interop_state.d11_12_fence.fence12,
                    self.present_index.saturating_sub(1),
                )
                .unwrap();

            buffer.previous_present_id = self.present_index as u64;

            self.present_index += 1;

            assert_eq!(self.presentation_manager.GetNextPresentId(), self.present_index);
            self.buffer_index = (self.buffer_index + 1) % self.buffers.len() as u64;

            self.poll_queries();
        }
    }
}

impl DCompSwapchain {
    fn wait_for_present(&mut self, id: u64) {
        unsafe {
            ResetEvent(*self.reset_event).unwrap();
            self.retiring_fence.SetEventOnCompletion(id, *self.reset_event).unwrap();
            let wait_handles = [self.lost_event, *self.reset_event];
            let wait_result = WaitForMultipleObjects(&wait_handles, FALSE, INFINITE);

            if wait_result == WAIT_OBJECT_0 {
                panic!("Lost event");
            }
            if wait_result.0 != WAIT_OBJECT_0.0 + 1 {
                let last_error = GetLastError();
                panic!("Unexpected wait result: {:0x}, error {:0x}", wait_result.0, last_error.0);
            }
        }
    }

    fn poll_queries(&mut self) {
        self.queries.retain_mut(|query| unsafe {
            let Some(start) =
                query::d3d11_query_get_data::<u64>(&self.d3d11_context, &query.start_query)
            else {
                return true;
            };
            let Some(end) =
                query::d3d11_query_get_data::<u64>(&self.d3d11_context, &query.end_query)
            else {
                return true;
            };

            query.tracy_query.take().unwrap().upload_timestamp(start as i64, end as i64);
            false
        });
    }
}

fn create_presentation_factory(
    d3d11_device: &ID3D11Device,
) -> Result<IPresentationFactory, anyhow::Error> {
    unsafe {
        let mut presentation_factory: *mut c_void = std::ptr::null_mut();
        CreatePresentationFactory(
            d3d11_device,
            &IPresentationFactory::IID,
            &mut presentation_factory as *mut *mut _ as *mut *mut c_void,
        )
        .context("Failed to create presentation factory")?;
        anyhow::ensure!(!presentation_factory.is_null(), "Presentation factory is null");
        Ok(IPresentationFactory::from_raw(presentation_factory))
    }
}

struct PresentationBuffer {
    texture: ID3D11Texture2D,
    buffer: IPresentationBuffer,
    previous_present_id: u64,
}

fn create_texture(
    presentation_manager: &IPresentationManager,
    d3d11_device: &ID3D11Device,
    size: UVec2,
    displayable_textures: bool,
) -> anyhow::Result<PresentationBuffer> {
    unsafe {
        let mut resource_flags = D3D11_RESOURCE_MISC_SHARED | D3D11_RESOURCE_MISC_SHARED_NTHANDLE;

        if displayable_textures {
            resource_flags |= D3D11_RESOURCE_MISC_SHARED_DISPLAYABLE;
        }

        let desc = D3D11_TEXTURE2D_DESC {
            Width: size.x,
            Height: size.y,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET).0 as _,
            CPUAccessFlags: 0,
            MiscFlags: resource_flags.0 as u32,
        };

        let mut texture = None;
        d3d11_device
            .CreateTexture2D(&desc, None, Some(&mut texture))
            .context("Failed to create texture")?;
        let texture = texture.unwrap();

        let buffer = presentation_manager
            .AddBufferFromResource(&texture)
            .context("Failed to create buffer from handle")?;

        Ok(PresentationBuffer { texture, buffer, previous_present_id: 0 })
    }
}

fn get_interrupt_time() -> u64 {
    unsafe { QueryInterruptTime() }
}
