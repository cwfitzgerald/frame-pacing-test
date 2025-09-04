use core::fmt;
use std::{any::Any, f32, mem, os::raw::c_void, ptr, slice, sync::Arc};

use anyhow::Context;
use arrayvec::ArrayVec;
use common::{assets::Asset, AssetTextureFormat};
use glam::Mat4;
use gpu_allocator::{
    d3d12::{Allocator, AllocatorCreateDesc, ID3D12DeviceVersion},
    AllocationSizes, AllocatorDebugSettings,
};
use parking_lot::Mutex;
use windows::{
    core::*,
    Win32::{
        Foundation::*,
        Graphics::{
            Direct3D::*,
            Direct3D12::*,
            Dxgi::{Common::*, *},
        },
    },
};

use crate::{
    data, include_shader, query,
    resource::{self, BufferViewType, RESOURCE_HEAP_SIZE},
    swapchain::{DCompSwapchain, DXGISwapchain, Swapchain},
    util::{self, transition_barrier, ID3D12ObjectExt, InterfaceExt},
    TextureDimension, TexturePointer,
};

pub mod egui;
mod environment;
mod tonemap;

pub(crate) const FRAMES_IN_FLIGHT: usize = 2;

const CONSTANT_BUFFER_SIZE: u64 = mem::size_of::<data::UniformData>() as _;

const MAX_OBJECTS: u64 = 1024;
const OBJECT_DATA_SIZE: u64 = mem::size_of::<data::ObjectData>() as _;
const OBJECT_BUFFER_SIZE: u64 = MAX_OBJECTS * OBJECT_DATA_SIZE;

const MAX_LIGHTS: u64 = 1024;
const LIGHT_DATA_SIZE: u64 = mem::size_of::<data::LightData>() as _;
const LIGHT_BUFFER_SIZE: u64 = MAX_LIGHTS * LIGHT_DATA_SIZE;

const MAX_VERTICES: u64 = 2_097_152;
const VERTEX_DATA_SIZE: u64 = mem::size_of::<data::VertexData>() as _;
const VERTEX_BUFFER_SIZE: u64 = MAX_VERTICES * VERTEX_DATA_SIZE;

const MAX_INDICES: u64 = 4_194_304;
const INDEX_DATA_SIZE: u64 = mem::size_of::<u32>() as _;
const INDEX_BUFFER_SIZE: u64 = MAX_INDICES * INDEX_DATA_SIZE;

const TOTAL_MESH_BUFFER_SIZE: u64 = VERTEX_BUFFER_SIZE + INDEX_BUFFER_SIZE;
struct PendingTexture {
    descriptor: Asset<crate::TextureDescriptor>,
    texture: resource::Texture,
}

pub(crate) struct PendingMeshData {
    pub descriptor: Arc<crate::MeshDescriptor>,
    pub destination_offset: u64,
}

#[derive(Clone, Copy)]
pub(crate) struct FrameIndex(u32);

impl FrameIndex {
    pub(crate) fn from_swapchain(swapchain: &dyn Swapchain) -> Self {
        Self((swapchain.frame_index() % FRAMES_IN_FLIGHT as u64) as u32)
    }

    pub(crate) fn get_gpu_index(&self) -> u32 {
        self.0
    }

    pub(crate) fn get_cpu_index(&self) -> u32 {
        self.0
    }
}

impl fmt::Debug for FrameIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameIndex")
            .field("gpu_index", &self.get_gpu_index())
            .field("cpu_index", &self.get_cpu_index())
            .finish()
    }
}

struct TextureUploadContext<'a> {
    device: &'a ID3D12Device,
    allocator: &'a Arc<Mutex<Allocator>>,
    primary_heap: &'a mut resource::DescriptorHeap,
    pending_textures: &'a mut Vec<PendingTexture>,
}

struct FrameSet {
    frame: ArrayVec<FrameData, FRAMES_IN_FLIGHT>,
    swapchain: ArrayVec<SwapchainData, FRAMES_IN_FLIGHT>,
}

impl FrameSet {
    fn get(&mut self, swapchain: &dyn Swapchain) -> (&mut FrameData, &mut SwapchainData) {
        let frame_number = swapchain.frame_index();

        let frame_count = self.frame.len();
        let frame = &mut self.frame[frame_number as usize % frame_count];
        let swapchain_count = self.swapchain.len();
        let swapchain = &mut self.swapchain[frame_number as usize % swapchain_count];
        (frame, swapchain)
    }
}

struct FrameData {
    fence_value: u64,
    graphics_command_allocator: ID3D12CommandAllocator,
    graphics_command_list: ID3D12GraphicsCommandList7,
    copy_command_allocator: ID3D12CommandAllocator,
    copy_command_list: ID3D12GraphicsCommandList7,
    graphics_query_manager: query::QueryManager,
    copy_query_manager: query::QueryManager,
    upload_scratch: resource::StagingBuffer,
    to_destroy: Vec<Box<dyn Any>>,
}

struct SwapchainData {
    render_target: ID3D12Resource2,
    rtv: resource::Descriptor,
}

pub struct Renderer {
    dxgi_debug: Option<IDXGIDebug1>,
    device: ID3D12Device8,
    graphics_command_queue: ID3D12CommandQueue,
    copy_command_queue: ID3D12CommandQueue,

    allocator: Arc<Mutex<Allocator>>,

    mesh_data_buffer: resource::Buffer,
    mesh_data_allocator: offset_allocator::Allocator,

    uniform_buffer: resource::OverwritingBufferBelt,
    object_buffer: resource::OverwritingBufferBelt,
    light_buffer: resource::OverwritingBufferBelt,

    swapchain: Box<dyn Swapchain>,

    graphics_fence: ID3D12Fence1,
    copy_fence: ID3D12Fence1,
    fence_value: u64,

    frames: FrameSet,

    primary_heap: resource::DescriptorHeap,
    rtv_heap: resource::DescriptorHeap,
    dsv_heap: resource::DescriptorHeap,

    root_signature: ID3D12RootSignature,
    pipeline_state: ID3D12PipelineState,

    pending_mesh_data: Vec<PendingMeshData>,
    pending_textures: Vec<PendingTexture>,
    loaded_textures: Vec<resource::Texture>,

    hdr_texture: resource::Texture,
    depth_texture: resource::Texture,

    tonemapper: tonemap::Tonemapper,
    environment: environment::EnvironmentMapper,
    egui_renderer: egui::EguiRenderer,

    size: glam::UVec2,
}

impl Renderer {
    pub fn new(
        hwnd: HWND,
        size: glam::UVec2,
        use_dcomp: bool,
        use_adapter: u32,
    ) -> anyhow::Result<Self> {
        unsafe {
            let _span = tracy_client::span!("Renderer::new");

            let mut dxgi_debug: Option<IDXGIDebug1> = None;
            let mut d3d12_debug: Option<ID3D12Debug6> = None;
            if cfg!(debug_assertions) {
                dxgi_debug = Some(DXGIGetDebugInterface1(0).unwrap());

                D3D12GetDebugInterface(&mut d3d12_debug).unwrap();
                if let Some(ref debug) = d3d12_debug {
                    debug.EnableDebugLayer();
                    // debug.SetEnableGPUBasedValidation(true);
                }
            }

            let factory_flags = if cfg!(debug_assertions) {
                DXGI_CREATE_FACTORY_DEBUG
            } else {
                DXGI_CREATE_FACTORY_FLAGS::default()
            };

            let factory: IDXGIFactory7 =
                CreateDXGIFactory2(factory_flags).context("Failed to create DXGI factory")?;

            let mut adapter_idx = 0;
            let mut adapters = Vec::new();
            while let Ok(adapter) = factory.EnumAdapterByGpuPreference::<IDXGIAdapter4>(
                adapter_idx,
                DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE,
            ) {
                let desc = adapter.GetDesc3().context("Failed to get adapter description")?;

                adapters.push((adapter, desc));
                adapter_idx += 1;
            }

            for (idx, (_, desc)) in adapters.iter().enumerate().skip(use_adapter as usize) {
                log::info!("Found Adapter {idx} {}", util::string_from_utf16(&desc.Description));
            }

            let mut opt_adapter = None;
            let mut desc = DXGI_ADAPTER_DESC3::default();
            let mut device: Option<ID3D12Device8> = None;
            for (idx, (adapter, adap_desc)) in
                adapters.into_iter().enumerate().skip(use_adapter as usize)
            {
                if let Err(e) = D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_11_0, &mut device) {
                    log::info!("Failed to get device from adapter {idx}: {e:?}");
                    continue;
                }
                opt_adapter = Some(adapter);
                desc = adap_desc;
                log::info!("Made a device from adapter {idx}!");
                break;
            }
            let adapter = opt_adapter.context("Failed to find a D3D12 compatible adapter")?;
            let device = device.context("Failed to find a D3D12 compatible device")?;

            let info_queue = device.cast::<ID3D12InfoQueue1>().ok();

            if let Some(ref info_queue) = info_queue {
                info_queue.SetBreakOnSeverity(D3D12_MESSAGE_SEVERITY_CORRUPTION, true).unwrap();
                info_queue.SetBreakOnSeverity(D3D12_MESSAGE_SEVERITY_ERROR, true).unwrap();
                info_queue
                    .SetBreakOnID(D3D12_MESSAGE_ID_WRITE_COMBINE_PERFORMANCE_WARNING, false)
                    .unwrap();

                let mut cookie = 0;

                info_queue
                    .RegisterMessageCallback(
                        Some(util::debug_callback),
                        D3D12_MESSAGE_CALLBACK_FLAG_NONE,
                        ptr::null_mut(),
                        &mut cookie,
                    )
                    .unwrap();

                if cookie == 0 {
                    log::warn!("Failed to register debug callback");
                }
            }

            let root_signature_version = util::check_root_signature_version(&device);
            let root_signature_version_str =
                util::format_root_signature_version(root_signature_version);

            let shader_model = util::check_shader_models(&device);
            let shader_model_str = util::format_shader_model(shader_model);

            let feature_level = util::check_feature_levels(&device);
            let feature_level_str = util::format_feature_level(feature_level);

            let options0: D3D12_FEATURE_DATA_D3D12_OPTIONS =
                util::check_feature_support(&device, D3D12_FEATURE_D3D12_OPTIONS);

            let options12: D3D12_FEATURE_DATA_D3D12_OPTIONS12 =
                util::check_feature_support(&device, D3D12_FEATURE_D3D12_OPTIONS12);

            let raw_driver_version = adapter.CheckInterfaceSupport(&IDXGIDevice::IID).unwrap_or(0);
            let mut driver_version: [u16; 4] = bytemuck::cast(raw_driver_version);
            driver_version.reverse();

            log::info!("Initialized D3D12 Device");
            log::info!("- Name: {}", util::string_from_utf16(&desc.Description));
            log::info!("- Feature Level: {}", feature_level_str);
            log::info!("- Shader Model: {}", shader_model_str);
            log::info!("- Resource Binding Tier: {}", options0.ResourceBindingTier.0);
            log::info!("- Root Signature Version: {}", root_signature_version_str);
            log::info!("- Enhanced Barriers: {}", options12.EnhancedBarriersSupported == TRUE);
            log::info!(
                "- Driver Version: {}.{}.{}.{}",
                driver_version[0],
                driver_version[1],
                driver_version[2],
                driver_version[3]
            );

            anyhow::ensure!(
                feature_level.0 >= D3D_FEATURE_LEVEL_12_0.0,
                "Feature Level 12.1 required, this device has {}",
                feature_level_str
            );
            anyhow::ensure!(
                shader_model.0 >= D3D_SHADER_MODEL_6_2.0,
                "Shader Model 6.6 required, this device has {}",
                shader_model_str
            );
            anyhow::ensure!(
                root_signature_version.0 >= D3D_ROOT_SIGNATURE_VERSION_1_1.0,
                "Root Signature Version 1.1 required, this device has {}",
                root_signature_version_str
            );
            anyhow::ensure!(
                options0.ResourceBindingTier.0 >= D3D12_RESOURCE_BINDING_TIER_3.0,
                "Resource binding Tier 3 required, this device has {}",
                options0.ResourceBindingTier.0
            );

            let allocator = Allocator::new(&AllocatorCreateDesc {
                device: ID3D12DeviceVersion::Device(ID3D12Device::clone(&device)),
                debug_settings: AllocatorDebugSettings::default(),
                allocation_sizes: AllocationSizes::default(),
            })
            .context("Failed to create allocator")?;
            let allocator = Arc::new(Mutex::new(allocator));

            let mut primary_heap =
                resource::DescriptorHeap::new(&device, D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV)
                    .context("Failed to create primary descriptor heap")?;

            let mut rtv_heap =
                resource::DescriptorHeap::new(&device, D3D12_DESCRIPTOR_HEAP_TYPE_RTV)
                    .context("Failed to create RTV heap")?;

            let mut dsv_heap =
                resource::DescriptorHeap::new(&device, D3D12_DESCRIPTOR_HEAP_TYPE_DSV)
                    .context("Failed to create DSV heap")?;

            let mesh_data_buffer = resource::Buffer::new(
                &device,
                &allocator,
                gpu_allocator::MemoryLocation::GpuOnly,
                TOTAL_MESH_BUFFER_SIZE,
                false,
                "Vertex Buffer",
            )
            .context("Failed to create mesh data buffer")?;

            let uniform_buffer = resource::OverwritingBufferBelt::new(
                &device,
                &allocator,
                CONSTANT_BUFFER_SIZE,
                "Uniform Buffer",
            )
            .context("Failed to create uniform buffer")?;

            let mut object_buffer = resource::OverwritingBufferBelt::new(
                &device,
                &allocator,
                OBJECT_BUFFER_SIZE,
                "Object Buffer",
            )
            .context("Failed to create object buffer")?;

            let mut light_buffer = resource::OverwritingBufferBelt::new(
                &device,
                &allocator,
                LIGHT_BUFFER_SIZE,
                "Light Buffer",
            )
            .context("Failed to create light buffer")?;

            let object_buffer_desc_index = object_buffer.create_descriptors(
                &device,
                &mut primary_heap,
                BufferViewType::Typed { stride: OBJECT_DATA_SIZE as _ },
            );

            assert_eq!(object_buffer_desc_index, [0, 1]);

            let vertex_buffer_desc_index = mesh_data_buffer.create_descriptor(
                &device,
                &mut primary_heap,
                VERTEX_DATA_SIZE as _,
            );

            assert_eq!(vertex_buffer_desc_index, 2);

            let light_buffer_desc_index = light_buffer.create_descriptors(
                &device,
                &mut primary_heap,
                BufferViewType::Typed { stride: LIGHT_DATA_SIZE as _ },
            );

            assert_eq!(light_buffer_desc_index, [3, 4]);

            let mesh_data_allocator =
                offset_allocator::Allocator::new(TOTAL_MESH_BUFFER_SIZE as u32);

            let graphics_fence: ID3D12Fence1 =
                device.CreateFence(0, D3D12_FENCE_FLAG_NONE).context("Failed to create fence")?;
            graphics_fence.set_name("Graphics Fence");
            let copy_fence: ID3D12Fence1 =
                device.CreateFence(0, D3D12_FENCE_FLAG_NONE).context("Failed to create fence")?;
            copy_fence.set_name("Copy Fence");

            let graphics_command_queue: ID3D12CommandQueue = device
                .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                    Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                    ..Default::default()
                })
                .context("Failed to create command queue")?;
            graphics_command_queue.set_name("Graphics Command Queue");
            let copy_command_queue: ID3D12CommandQueue = device
                .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                    Type: D3D12_COMMAND_LIST_TYPE_COPY,
                    ..Default::default()
                })
                .context("Failed to create copy command queue")?;
            copy_command_queue.set_name("Copy Command Queue");

            let swapchain: Box<dyn Swapchain> = if use_dcomp {
                Box::new(DCompSwapchain::new(
                    &adapter,
                    &device,
                    &graphics_command_queue,
                    hwnd,
                    size,
                )?)
            } else {
                Box::new(DXGISwapchain::new(&factory, &graphics_command_queue, hwnd, size)?)
            };

            let query_contexts =
                query::QueryContexts::new(&graphics_command_queue, &copy_command_queue)
                    .context("Failed to create query contexts")?;

            let mut frames = FrameSet { frame: ArrayVec::new(), swapchain: ArrayVec::new() };
            for i in 0..FRAMES_IN_FLIGHT {
                // let render_target: ID3D12Resource2 = swapchain
                //     .GetBuffer(i as _)
                //     .with_context(|| format!("Failed to get swapchain buffer {}", i))?;

                // let rtv_desc_handle = rtv_heap.allocate(
                //     &device,
                //     &render_target,
                //     resource::DescriptorType::Rtv { format: DXGI_FORMAT_R8G8B8A8_UNORM_SRGB },
                // );

                let graphics_command_allocator: ID3D12CommandAllocator = device
                    .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)
                    .with_context(|| format!("Failed to create command allocator {}", i))?;
                graphics_command_allocator.set_name(&format!("Graphics Command Allocator {}", i));
                let copy_command_allocator: ID3D12CommandAllocator = device
                    .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COPY)
                    .with_context(|| format!("Failed to create copy command allocator {}", i))?;
                copy_command_allocator.set_name(&format!("Copy Command Allocator {}", i));

                let graphics_command_list: ID3D12GraphicsCommandList7 = device
                    .CreateCommandList(
                        0,
                        D3D12_COMMAND_LIST_TYPE_DIRECT,
                        &graphics_command_allocator,
                        None,
                    )
                    .with_context(|| format!("Failed to create command list {}", i))?;
                graphics_command_list.set_name(&format!("Graphics Command List {}", i));
                let copy_command_list: ID3D12GraphicsCommandList7 = device
                    .CreateCommandList(
                        0,
                        D3D12_COMMAND_LIST_TYPE_COPY,
                        &copy_command_allocator,
                        None,
                    )
                    .with_context(|| format!("Failed to create copy command list {}", i))?;
                copy_command_list.set_name(&format!("Copy Command List {}", i));

                graphics_command_list.Close().unwrap();
                copy_command_list.Close().unwrap();

                let graphics_query_manager = query::QueryManager::new(
                    &device,
                    &allocator,
                    graphics_command_list.clone().into(),
                    query_contexts.graphics.clone(),
                    &format!("Graphics Query Manager {i}"),
                )
                .context("Failed to create graphics query manager")?;
                let copy_query_manager = query::QueryManager::new(
                    &device,
                    &allocator,
                    copy_command_list.clone().into(),
                    query_contexts.copy.clone(),
                    &format!("Copy Query Manager {i}"),
                )
                .context("Failed to create copy query manager")?;

                let upload_scratch = resource::StagingBuffer::new(
                    &device,
                    &allocator,
                    64 * 1024 * 1024,
                    "Upload Staging Buffer",
                )
                .context("Failed to create upload scratch buffer")?;

                frames.frame.push(FrameData {
                    fence_value: 0,
                    graphics_command_allocator,
                    graphics_command_list,
                    copy_command_allocator,
                    copy_command_list,
                    graphics_query_manager,
                    copy_query_manager,
                    upload_scratch,
                    to_destroy: Vec::new(),
                });
            }

            for buffer in swapchain.get_buffers() {
                let rtv_desc_handle = rtv_heap.allocate(
                    &device,
                    &buffer,
                    resource::DescriptorType::Rtv { format: DXGI_FORMAT_R8G8B8A8_UNORM_SRGB },
                );

                frames.swapchain.push(SwapchainData {
                    render_target: buffer.cast().unwrap(),
                    rtv: rtv_desc_handle,
                });
            }

            let mut depth_texture = resource::Texture::new(
                &device,
                &allocator,
                resource::DescriptorType::Dsv { format: DXGI_FORMAT_D32_FLOAT },
                size,
                "Depth Texture",
            )
            .context("Failed to construct depth texture")?;
            depth_texture.create_descriptor(
                &device,
                &mut dsv_heap,
                resource::DescriptorType::Dsv { format: DXGI_FORMAT_D32_FLOAT },
                None,
            );

            let mut hdr_texture = resource::Texture::new(
                &device,
                &allocator,
                resource::DescriptorType::Rtv { format: DXGI_FORMAT_R16G16B16A16_FLOAT },
                size,
                "HDR Texture",
            )
            .context("Failed to construct HDR texture")?;
            hdr_texture.create_descriptor(
                &device,
                &mut rtv_heap,
                resource::DescriptorType::Rtv { format: DXGI_FORMAT_R16G16B16A16_FLOAT },
                None,
            );
            hdr_texture.create_descriptor(
                &device,
                &mut primary_heap,
                resource::DescriptorType::Srv2DImage {
                    format: DXGI_FORMAT_R16G16B16A16_FLOAT,
                    mipmaps: 1,
                },
                None,
            );

            let root_signature: ID3D12RootSignature = {
                let bound_range = [
                    // Various buffers
                    D3D12_DESCRIPTOR_RANGE1 {
                        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
                        NumDescriptors: 5,
                        BaseShaderRegister: 0,
                        RegisterSpace: 0,
                        OffsetInDescriptorsFromTableStart: 0,
                        Flags: D3D12_DESCRIPTOR_RANGE_FLAG_DATA_STATIC_WHILE_SET_AT_EXECUTE,
                    },
                ];

                let bindless_range = [
                    // Bindless range
                    D3D12_DESCRIPTOR_RANGE1 {
                        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
                        NumDescriptors: RESOURCE_HEAP_SIZE as _,
                        BaseShaderRegister: 0,
                        RegisterSpace: 1,
                        OffsetInDescriptorsFromTableStart: 0,
                        Flags: D3D12_DESCRIPTOR_RANGE_FLAG_DESCRIPTORS_VOLATILE,
                    },
                ];

                let parameters = [
                    D3D12_ROOT_PARAMETER1 {
                        ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
                        ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
                        Anonymous: D3D12_ROOT_PARAMETER1_0 {
                            Constants: D3D12_ROOT_CONSTANTS {
                                ShaderRegister: 0,
                                RegisterSpace: 0,
                                Num32BitValues: 2,
                            },
                        },
                    },
                    D3D12_ROOT_PARAMETER1 {
                        ParameterType: D3D12_ROOT_PARAMETER_TYPE_CBV,
                        ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
                        Anonymous: D3D12_ROOT_PARAMETER1_0 {
                            Descriptor: D3D12_ROOT_DESCRIPTOR1 {
                                ShaderRegister: 1,
                                RegisterSpace: 0,
                                Flags: D3D12_ROOT_DESCRIPTOR_FLAG_DATA_VOLATILE,
                            },
                        },
                    },
                    D3D12_ROOT_PARAMETER1 {
                        ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
                        ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
                        Anonymous: D3D12_ROOT_PARAMETER1_0 {
                            DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE1 {
                                NumDescriptorRanges: bound_range.len() as _,
                                pDescriptorRanges: bound_range.as_ptr(),
                            },
                        },
                    },
                    D3D12_ROOT_PARAMETER1 {
                        ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
                        ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
                        Anonymous: D3D12_ROOT_PARAMETER1_0 {
                            DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE1 {
                                NumDescriptorRanges: bindless_range.len() as _,
                                pDescriptorRanges: bindless_range.as_ptr(),
                            },
                        },
                    },
                ];

                let sampler = D3D12_STATIC_SAMPLER_DESC {
                    Filter: D3D12_FILTER_ANISOTROPIC,
                    AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
                    AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
                    AddressW: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
                    MipLODBias: 0.0,
                    MaxAnisotropy: 16,
                    ComparisonFunc: D3D12_COMPARISON_FUNC_NONE,
                    BorderColor: D3D12_STATIC_BORDER_COLOR_TRANSPARENT_BLACK,
                    MinLOD: 0.0,
                    MaxLOD: D3D12_FLOAT32_MAX,
                    ShaderRegister: 0,
                    RegisterSpace: 0,
                    ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
                };

                create_root_signature(&device, &parameters, &[sampler])?
            };

            let vs_dxil = include_shader!(Vertex "triangle");
            let ps_dxil = include_shader!(Pixel "triangle");

            let pipeline_desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
                pRootSignature: root_signature.to_option_unowned(),
                VS: D3D12_SHADER_BYTECODE {
                    pShaderBytecode: vs_dxil.as_ptr() as *const c_void,
                    BytecodeLength: vs_dxil.len(),
                },
                PS: D3D12_SHADER_BYTECODE {
                    pShaderBytecode: ps_dxil.as_ptr() as *const c_void,
                    BytecodeLength: ps_dxil.len(),
                },
                BlendState: {
                    let mut blend_state = D3D12_BLEND_DESC::default();
                    blend_state.RenderTarget[0].RenderTargetWriteMask =
                        D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8;
                    blend_state
                },
                SampleMask: u32::MAX,
                RasterizerState: D3D12_RASTERIZER_DESC {
                    FillMode: D3D12_FILL_MODE_SOLID,
                    CullMode: D3D12_CULL_MODE_BACK,
                    FrontCounterClockwise: TRUE,
                    DepthClipEnable: TRUE,
                    ..Default::default()
                },
                DepthStencilState: D3D12_DEPTH_STENCIL_DESC {
                    DepthEnable: TRUE,
                    DepthWriteMask: D3D12_DEPTH_WRITE_MASK_ALL,
                    DepthFunc: D3D12_COMPARISON_FUNC_GREATER_EQUAL,
                    StencilEnable: FALSE,
                    ..Default::default()
                },
                DSVFormat: DXGI_FORMAT_D32_FLOAT,
                InputLayout: D3D12_INPUT_LAYOUT_DESC::default(),
                PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
                NumRenderTargets: 1,
                RTVFormats: {
                    let mut formats = [DXGI_FORMAT::default(); 8];
                    formats[0] = DXGI_FORMAT_R16G16B16A16_FLOAT;
                    formats
                },
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, ..Default::default() },
                ..Default::default()
            };

            let pipeline_state: ID3D12PipelineState = device
                .CreateGraphicsPipelineState(&pipeline_desc)
                .context("Failed to create pipeline state")?;
            pipeline_state.set_name("Triangle State");

            let mut pending_textures = Vec::new();
            let mut texture_upload_context = TextureUploadContext {
                device: &device,
                allocator: &allocator,
                primary_heap: &mut primary_heap,
                pending_textures: &mut pending_textures,
            };

            let tonemapper = tonemap::Tonemapper::new(&mut texture_upload_context)?;
            let environment = environment::EnvironmentMapper::new(&device)?;
            let egui_renderer = egui::EguiRenderer::new(&device, &allocator)?;

            Ok(Self {
                dxgi_debug,
                device,
                graphics_command_queue,
                copy_command_queue,

                allocator,
                mesh_data_buffer,
                mesh_data_allocator,
                uniform_buffer,
                object_buffer,
                light_buffer,

                swapchain,
                // waitable_object,
                graphics_fence,
                copy_fence,
                fence_value: 1,
                frames,

                primary_heap,
                rtv_heap,
                dsv_heap,

                root_signature,
                pipeline_state,

                hdr_texture,
                depth_texture,

                pending_mesh_data: Vec::new(),
                pending_textures,
                loaded_textures: Vec::new(),

                tonemapper,
                environment,
                egui_renderer,

                size,
                // _vsync_thread: vsync_thread,
            })
        }
    }

    pub fn resize(&mut self, size: glam::UVec2) -> anyhow::Result<()> {
        unsafe {
            let _span = tracy_client::span!("Renderer::resize");

            // Wait for idle
            let wait_for_idle_span = tracy_client::span!("Wait for idle");
            self.graphics_command_queue.Signal(&self.graphics_fence, self.fence_value).unwrap();
            self.graphics_fence.SetEventOnCompletion(self.fence_value, None).unwrap();
            drop(wait_for_idle_span);

            // Resize the swapchain

            let rtv_descriptors: ArrayVec<_, 2> =
                self.frames.swapchain.drain(..).map(|swapchain_data| swapchain_data.rtv).collect();

            self.swapchain.resize(&self.device, size);

            // Recreate the RTVs

            let buffers = self.swapchain.get_buffers();
            for (rtv, buffer) in rtv_descriptors.into_iter().zip(&buffers) {
                self.rtv_heap.rewrite(
                    &self.device,
                    &buffer,
                    resource::DescriptorType::Rtv { format: DXGI_FORMAT_R8G8B8A8_UNORM_SRGB },
                    &rtv,
                );
                self.frames
                    .swapchain
                    .push(SwapchainData { render_target: buffer.cast().unwrap(), rtv });
            }

            // Recreate the HDR texture
            let hdr_srv_descriptor = self.hdr_texture.srv_descriptor().clone();
            let hdr_rtv_descriptor = self.hdr_texture.rtv_descriptor().clone();
            self.hdr_texture = resource::Texture::new(
                &self.device,
                &self.allocator,
                resource::DescriptorType::Rtv { format: DXGI_FORMAT_R16G16B16A16_FLOAT },
                size,
                "HDR Texture",
            )?;
            self.hdr_texture.create_descriptor(
                &self.device,
                &mut self.primary_heap,
                resource::DescriptorType::Srv2DImage {
                    format: DXGI_FORMAT_R16G16B16A16_FLOAT,
                    mipmaps: 1,
                },
                Some(&hdr_srv_descriptor),
            );
            self.hdr_texture.create_descriptor(
                &self.device,
                &mut self.rtv_heap,
                resource::DescriptorType::Rtv { format: DXGI_FORMAT_R16G16B16A16_FLOAT },
                Some(&hdr_rtv_descriptor),
            );

            // Recreate the depth texture

            let depth_descriptor = self.depth_texture.dsv_descriptor().clone();
            self.depth_texture = resource::Texture::new(
                &self.device,
                &self.allocator,
                resource::DescriptorType::Dsv { format: DXGI_FORMAT_D32_FLOAT },
                size,
                "Depth Texture",
            )?;
            self.depth_texture.create_descriptor(
                &self.device,
                &mut self.dsv_heap,
                resource::DescriptorType::Dsv { format: DXGI_FORMAT_D32_FLOAT },
                Some(&depth_descriptor),
            );

            // Update the state

            self.size = size;
            self.fence_value += 1;

            Ok(())
        }
    }

    pub(crate) fn upload_mesh(
        &mut self,
        mesh: Arc<crate::MeshDescriptor>,
    ) -> crate::MeshDataPointers {
        let _span = tracy_client::span!("Renderer::upload_mesh");

        let index_count = mesh.index_data.len() as u32;

        let vertex_data_needed = mesh.vertex_data.len() as u64 * VERTEX_DATA_SIZE;
        let index_data_needed = mesh.index_data.len() as u64 * INDEX_DATA_SIZE;
        let data_needed = vertex_data_needed + index_data_needed;
        let data_needed = data_needed.next_multiple_of(VERTEX_DATA_SIZE);

        let destination_offset =
            self.mesh_data_allocator.allocate(data_needed as u32).unwrap().offset as u64;

        self.pending_mesh_data.push(PendingMeshData { descriptor: mesh, destination_offset });

        let vertex_offset_bytes = destination_offset;
        let index_offset_bytes = destination_offset + vertex_data_needed;

        let vertex_offset = vertex_offset_bytes / VERTEX_DATA_SIZE;
        let index_offset = index_offset_bytes / INDEX_DATA_SIZE;

        crate::MeshDataPointers { vertex_offset, index_offset, index_count }
    }

    pub(crate) fn upload_image(
        &mut self,
        descriptor: Asset<crate::TextureDescriptor>,
    ) -> TexturePointer {
        let mut ctx: TextureUploadContext = TextureUploadContext {
            device: &self.device,
            allocator: &self.allocator,
            primary_heap: &mut self.primary_heap,
            pending_textures: &mut self.pending_textures,
        };

        Self::upload_image_impl(&mut ctx, descriptor)
    }

    fn upload_image_impl(
        ctx: &mut TextureUploadContext<'_>,
        descriptor: Asset<crate::TextureDescriptor>,
    ) -> TexturePointer {
        let _span = tracy_client::span!("Renderer::upload_image");

        let format = match descriptor.format {
            AssetTextureFormat::Rgba8 => DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
            AssetTextureFormat::Rgb9e5 => DXGI_FORMAT_R9G9B9E5_SHAREDEXP,
            AssetTextureFormat::BC1 => DXGI_FORMAT_BC1_UNORM_SRGB,
            AssetTextureFormat::BC5 => DXGI_FORMAT_BC5_UNORM,
            AssetTextureFormat::BC6 => DXGI_FORMAT_BC6H_UF16,
            AssetTextureFormat::BC7 => DXGI_FORMAT_BC7_UNORM_SRGB,
        };

        let descriptor_ty = match descriptor.dimension {
            TextureDimension::D2 => {
                resource::DescriptorType::Srv2DImage { format, mipmaps: descriptor.mipmaps }
            }
            TextureDimension::Cube => {
                resource::DescriptorType::SrvCubemap { format, mipmaps: descriptor.mipmaps }
            }
            TextureDimension::D3 => {
                resource::DescriptorType::Srv3DImage { format, depth: descriptor.depth }
            }
        };
        let mut texture = resource::Texture::new(
            ctx.device,
            ctx.allocator,
            descriptor_ty,
            descriptor.size,
            &descriptor.path,
        )
        .unwrap();
        texture.create_descriptor(ctx.device, ctx.primary_heap, descriptor_ty, None);

        let index = texture.srv_descriptor().index;

        ctx.pending_textures.push(PendingTexture { descriptor, texture });

        TexturePointer(index)
    }

    pub(crate) fn render(
        &mut self,
        environment: Option<&crate::TexturePointer>,
        view_matrix: Mat4,
        view_matrix_no_transform: Mat4,
        objects: &[crate::LoadedObjects],
        lights: &[crate::LightData],
        egui_input: egui::EguiRendererInput,
    ) {
        unsafe {
            let _span = tracy_client::span!("Renderer::render");

            let frame_index = FrameIndex::from_swapchain(&*self.swapchain);
            log::debug!("Starting frame {:#?}", frame_index);
            let (frame, swapchain) = self.frames.get(&*self.swapchain);

            // --- Wait for the previous frame to finish ---

            let span = tracy_client::span!("Wait for fence");
            log::debug!("CPU Waiting on fence {}", frame.fence_value);
            self.graphics_fence.SetEventOnCompletion(frame.fence_value, None).unwrap();
            drop(span);

            let span = tracy_client::span!("Wait for Swapchain");
            self.swapchain.acquire();
            drop(span);

            frame.graphics_query_manager.read();
            frame.copy_query_manager.read();

            frame.upload_scratch.clear();
            frame.to_destroy.clear();

            // --- Reset the command allocator and command list ---
            frame.graphics_command_allocator.Reset().unwrap();
            frame.graphics_command_list.Reset(&frame.graphics_command_allocator, None).unwrap();
            frame.copy_command_allocator.Reset().unwrap();
            frame.copy_command_list.Reset(&frame.copy_command_allocator, None).unwrap();

            // --- User Code ---

            self.egui_renderer.upload(
                &self.device,
                &self.allocator,
                &frame.graphics_command_list,
                &frame.copy_command_list,
                &mut self.primary_heap,
                frame_index,
                &mut frame.upload_scratch,
                &mut frame.to_destroy,
                &egui_input,
            );

            let proj_matrix = glam::Mat4::perspective_infinite_reverse_rh(
                60.0_f32.to_radians(),
                self.size.x.max(1) as f32 / self.size.y.max(1) as f32,
                0.01,
            ) * common::coords::WORLD_TO_CLIP_MATRIX;

            let view_projection_no_transform = proj_matrix
                * view_matrix_no_transform
                * glam::Mat4::from_rotation_x(f32::consts::FRAC_PI_2);
            let inv_view_projection_no_transform = view_projection_no_transform.inverse();

            let uniform = data::UniformData {
                proj: proj_matrix,
                inv_view_projection_no_transform,
                frame_number: frame_index.get_gpu_index(),
                light_count: lights.len() as u32,
                _padding: Default::default(),
            };

            let object_data: Vec<_> = objects
                .iter()
                .map(|o| data::ObjectData {
                    model_matrix: o.transform,
                    model_view_matrix: view_matrix * o.transform,
                    diffuse_texture: o.texture_data.diffuse.0,
                    metallic_roughnes_texture: o.texture_data.metallic_roughness.0,
                    _padding: Default::default(),
                })
                .collect();

            let light_data = lights
                .iter()
                .copied()
                .map(|mut l| {
                    l.position = view_matrix.transform_point3(l.position);
                    l
                })
                .collect::<Vec<_>>();

            let copy_span = frame.copy_query_manager.start_span("Upload Uniform Data");
            self.uniform_buffer.upload_data(
                frame_index,
                &frame.copy_command_list,
                bytemuck::bytes_of(&uniform),
            );
            frame.copy_query_manager.end_span(copy_span);

            let copy_span = frame.copy_query_manager.start_span("Upload Object Data");
            self.object_buffer.upload_data(
                frame_index,
                &frame.copy_command_list,
                bytemuck::cast_slice(&object_data),
            );
            frame.copy_query_manager.end_span(copy_span);

            let copy_span = frame.copy_query_manager.start_span("Upload Light Data");
            self.light_buffer.upload_data(
                frame_index,
                &frame.copy_command_list,
                bytemuck::cast_slice(&light_data),
            );
            frame.copy_query_manager.end_span(copy_span);

            let copy_pending_mesh_data_span = frame
                .copy_query_manager
                .start_span(&format!("Upload {} Pending Mesh Data", self.pending_mesh_data.len()));
            for pending_mesh_data in self.pending_mesh_data.drain(..) {
                frame.upload_scratch.upload_mesh(
                    &frame.copy_command_list,
                    self.mesh_data_buffer.resource(),
                    &pending_mesh_data,
                );
            }
            frame.copy_query_manager.end_span(copy_pending_mesh_data_span);

            let copy_pending_textures_span = frame
                .copy_query_manager
                .start_span(&format!("Upload {} Pending Textures", self.pending_textures.len()));
            for pending_texture in self.pending_textures.drain(..) {
                let texture = pending_texture.texture;
                let descriptor = &pending_texture.descriptor;

                log::info!("Uploading texture: {:?}", descriptor.path);

                frame.upload_scratch.upload_to_texture(
                    &texture,
                    &frame.copy_command_list,
                    None,
                    &descriptor.data,
                );

                self.loaded_textures.push(texture);
            }
            frame.copy_query_manager.end_span(copy_pending_textures_span);

            // --- Record the command list ---

            let graphics_span = frame.graphics_query_manager.start_span("Graphics");
            let viewport = D3D12_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: self.size.x as f32,
                Height: self.size.y as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            };
            let scissor_rect =
                RECT { left: 0, top: 0, right: self.size.x as i32, bottom: self.size.y as i32 };
            frame.graphics_command_list.RSSetViewports(slice::from_ref(&viewport));
            frame.graphics_command_list.RSSetScissorRects(slice::from_ref(&scissor_rect));

            frame.graphics_command_list.ClearRenderTargetView(
                self.hdr_texture.rtv_descriptor().handle,
                &[0.0, 0.0, 0.0, 1.0],
                None,
            );
            frame.graphics_command_list.ClearDepthStencilView(
                self.depth_texture.dsv_descriptor().handle,
                D3D12_CLEAR_FLAG_DEPTH,
                0.0,
                0,
                &[],
            );

            frame.graphics_command_list.OMSetRenderTargets(
                1,
                Some(&self.hdr_texture.rtv_descriptor().handle),
                false,
                Some(&self.depth_texture.dsv_descriptor().handle),
            );

            frame.graphics_command_list.SetDescriptorHeaps(&[Some(self.primary_heap.heap())]);
            frame.graphics_command_list.SetGraphicsRootSignature(&self.root_signature);
            let constant_buffer_address = self.uniform_buffer.gpu_address(frame_index);
            frame
                .graphics_command_list
                .SetGraphicsRootConstantBufferView(1, constant_buffer_address);
            frame
                .graphics_command_list
                .SetGraphicsRootDescriptorTable(2, self.primary_heap.base_gpu_handle());
            frame
                .graphics_command_list
                .SetGraphicsRootDescriptorTable(3, self.primary_heap.base_gpu_handle());
            frame.graphics_command_list.IASetIndexBuffer(Some(&D3D12_INDEX_BUFFER_VIEW {
                BufferLocation: self.mesh_data_buffer.gpu_address(),
                SizeInBytes: self.mesh_data_buffer.size() as _,
                Format: DXGI_FORMAT_R32_UINT,
            }));
            frame.graphics_command_list.SetPipelineState(&self.pipeline_state);
            frame.graphics_command_list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            for (idx, object) in objects.iter().enumerate() {
                frame.graphics_command_list.SetGraphicsRoot32BitConstant(0, idx as u32, 0);
                frame.graphics_command_list.SetGraphicsRoot32BitConstant(
                    0,
                    object.mesh_data.vertex_offset as u32,
                    1,
                );
                frame.graphics_command_list.DrawIndexedInstanced(
                    object.mesh_data.index_count,
                    1,
                    object.mesh_data.index_offset as u32,
                    0, // Vertex offset doesn't affect SV_VertexID in d3d12, it is shipped in the above root constant.
                    0,
                );
            }

            if let Some(environment) = environment {
                self.environment.render(
                    &frame.graphics_command_list,
                    &self.primary_heap,
                    constant_buffer_address,
                    *environment,
                );
            }

            frame.graphics_command_list.ResourceBarrier(&[
                transition_barrier(
                    &swapchain.render_target,
                    D3D12_RESOURCE_STATE_PRESENT,
                    D3D12_RESOURCE_STATE_RENDER_TARGET,
                ),
                transition_barrier(
                    self.hdr_texture.resource(),
                    D3D12_RESOURCE_STATE_RENDER_TARGET,
                    D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
                ),
            ]);

            frame.graphics_command_list.DiscardResource(&swapchain.render_target, None);
            frame.graphics_command_list.OMSetRenderTargets(
                1,
                Some(&swapchain.rtv.handle),
                FALSE,
                None,
            );

            self.tonemapper.render(
                &frame.graphics_command_list,
                &self.primary_heap,
                &self.hdr_texture,
            );

            self.egui_renderer.render(
                &frame.graphics_command_list,
                &self.primary_heap,
                frame_index,
                self.size,
            );

            frame.graphics_command_list.ResourceBarrier(&[
                transition_barrier(
                    &swapchain.render_target,
                    D3D12_RESOURCE_STATE_RENDER_TARGET,
                    D3D12_RESOURCE_STATE_PRESENT,
                ),
                transition_barrier(
                    self.hdr_texture.resource(),
                    D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
                    D3D12_RESOURCE_STATE_RENDER_TARGET,
                ),
            ]);
            frame.graphics_query_manager.end_span(graphics_span);

            self.egui_renderer.cleanup(&egui_input, &mut frame.to_destroy);

            // --- Execute the command list ---

            frame.graphics_query_manager.resolve();
            frame.copy_query_manager.resolve();

            frame.graphics_command_list.Close().unwrap();
            frame.copy_command_list.Close().unwrap();

            let previous_fence_value = self.fence_value.saturating_sub(2);

            log::debug!("Submitting: wait fence {}", previous_fence_value);
            log::debug!("Submitting: signal fence {}", self.fence_value);

            self.copy_command_queue.Wait(&self.graphics_fence, previous_fence_value).unwrap();
            let copy_command_lists = [Some(frame.copy_command_list.clone().into())];
            self.copy_command_queue.ExecuteCommandLists(&copy_command_lists);
            self.copy_command_queue.Signal(&self.copy_fence, self.fence_value).unwrap();

            self.graphics_command_queue.Wait(&self.copy_fence, self.fence_value).unwrap();
            let graphics_command_lists = [Some(frame.graphics_command_list.clone().into())];
            self.graphics_command_queue.ExecuteCommandLists(&graphics_command_lists);
            self.graphics_command_queue.Signal(&self.graphics_fence, self.fence_value).unwrap();

            frame.fence_value = self.fence_value;

            // --- Present the frame ---

            self.swapchain.present(&self.graphics_command_queue);

            // --- Prepare for the next frame ---

            self.fence_value += 1;
        }
    }

    pub fn destroy(self) {
        let dxgi_debug = self.dxgi_debug.clone();

        drop(self);

        if let Some(debug) = dxgi_debug {
            unsafe {
                debug.ReportLiveObjects(DXGI_DEBUG_ALL, DXGI_DEBUG_RLO_DETAIL).unwrap();
            }
        }
    }
}

fn create_root_signature(
    device: &ID3D12Device,
    parameters: &[D3D12_ROOT_PARAMETER1],
    samplers: &[D3D12_STATIC_SAMPLER_DESC],
) -> anyhow::Result<ID3D12RootSignature> {
    unsafe {
        let root_signature_desc = D3D12_ROOT_SIGNATURE_DESC1 {
            NumParameters: parameters.len() as _,
            pParameters: parameters.as_ptr(),
            NumStaticSamplers: samplers.len() as _,
            pStaticSamplers: samplers.as_ptr(),
            Flags: D3D12_ROOT_SIGNATURE_FLAG_NONE,
        };
        let versioned_root_sig_desc = D3D12_VERSIONED_ROOT_SIGNATURE_DESC {
            Version: D3D_ROOT_SIGNATURE_VERSION_1_1,
            Anonymous: D3D12_VERSIONED_ROOT_SIGNATURE_DESC_0 { Desc_1_1: root_signature_desc },
        };
        let mut root_sig_blob = None;
        let mut error = None;
        D3D12SerializeVersionedRootSignature(
            &versioned_root_sig_desc,
            &mut root_sig_blob,
            Some(&mut error),
        )
        .context("Failed to serialize root signature")?;
        if let Some(error) = error {
            anyhow::bail!("Error serializing root signature: {:?}", util::str_from_blob(&error));
        }
        Ok(device.CreateRootSignature(0, util::array_from_blob(&root_sig_blob.unwrap())).unwrap())
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            let _span = tracy_client::span!("Renderer::drop");

            self.graphics_command_queue.Signal(&self.graphics_fence, self.fence_value).unwrap();

            self.graphics_fence.SetEventOnCompletion(self.fence_value, None).unwrap();

            // end spans
            for frame in &mut self.frames.frame {
                frame.graphics_query_manager.read();
                frame.copy_query_manager.read();
            }

            // let _ = CloseHandle(self.waitable_object);
        };

        // We can't do ReportLiveObjects here but we can shut down correctly.
    }
}
