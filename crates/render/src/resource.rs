use std::{mem, sync::Arc};

use anyhow::Context;
use glam::UVec2;
use gpu_allocator::{
    d3d12::{Allocation, AllocationCreateDesc, Allocator},
    MemoryLocation,
};
use parking_lot::Mutex;
use windows::Win32::Graphics::{Direct3D12::*, Dxgi::Common::*};

use crate::{
    data::VertexData,
    renderer::{FrameIndex, PendingMeshData, FRAMES_IN_FLIGHT},
    util::{ID3D12ObjectExt, InterfaceExt},
    TexturePointer,
};

const MAX_RENDER_TARGETS: u32 = FRAMES_IN_FLIGHT as u32 + 1;
const MAX_DEPTH_TARGETS: u32 = 1;
pub const RESOURCE_HEAP_SIZE: u32 = 1024;
pub const SAMPLER_HEAP_SIZE: u32 = 32;

#[derive(Clone, Copy)]
pub enum DescriptorType {
    SrvBuffer { offset: u64, count: u32, stride: u32, raw: bool },
    Srv2DImage { format: DXGI_FORMAT, mipmaps: u16 },
    SrvCubemap { format: DXGI_FORMAT, mipmaps: u16 },
    Srv3DImage { format: DXGI_FORMAT, depth: u16 },
    Rtv { format: DXGI_FORMAT },
    Dsv { format: DXGI_FORMAT },
}

impl DescriptorType {
    fn format(&self) -> DXGI_FORMAT {
        match *self {
            DescriptorType::SrvBuffer { .. } => DXGI_FORMAT_UNKNOWN,
            DescriptorType::Srv2DImage { format, .. }
            | DescriptorType::SrvCubemap { format, .. }
            | DescriptorType::Srv3DImage { format, .. }
            | DescriptorType::Rtv { format }
            | DescriptorType::Dsv { format } => format,
        }
    }

    fn mipmaps(&self) -> u16 {
        match *self {
            DescriptorType::SrvBuffer { .. } => 1,
            DescriptorType::Srv2DImage { mipmaps, .. } => mipmaps,
            DescriptorType::SrvCubemap { mipmaps, .. } => mipmaps,
            DescriptorType::Srv3DImage { .. } => 1,
            DescriptorType::Rtv { .. } => 1,
            DescriptorType::Dsv { .. } => 1,
        }
    }

    fn array_layers(&self) -> u16 {
        match *self {
            DescriptorType::SrvBuffer { .. } => 1,
            DescriptorType::Srv2DImage { .. } => 1,
            DescriptorType::SrvCubemap { .. } => 6,
            DescriptorType::Srv3DImage { .. } => 1,
            DescriptorType::Rtv { .. } => 1,
            DescriptorType::Dsv { .. } => 1,
        }
    }

    fn depth(&self) -> u16 {
        match *self {
            DescriptorType::SrvBuffer { .. } => 1,
            DescriptorType::Srv2DImage { .. } => 1,
            DescriptorType::SrvCubemap { .. } => 1,
            DescriptorType::Srv3DImage { depth, .. } => depth,
            DescriptorType::Rtv { .. } => 1,
            DescriptorType::Dsv { .. } => 1,
        }
    }

    fn depth_or_array_layers(&self) -> u16 {
        match *self {
            DescriptorType::SrvBuffer { .. } => 1,
            DescriptorType::Srv2DImage { .. } => 1,
            DescriptorType::SrvCubemap { .. } => 6,
            DescriptorType::Srv3DImage { depth, .. } => depth,
            DescriptorType::Rtv { .. } => 1,
            DescriptorType::Dsv { .. } => 1,
        }
    }

    fn dimension(&self) -> D3D12_RESOURCE_DIMENSION {
        match *self {
            DescriptorType::SrvBuffer { .. } => D3D12_RESOURCE_DIMENSION_BUFFER,
            DescriptorType::Srv2DImage { .. } => D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            DescriptorType::SrvCubemap { .. } => D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            DescriptorType::Srv3DImage { .. } => D3D12_RESOURCE_DIMENSION_TEXTURE3D,
            DescriptorType::Rtv { .. } => D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            DescriptorType::Dsv { .. } => D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        }
    }

    fn initial_state(&self) -> D3D12_RESOURCE_STATES {
        match *self {
            DescriptorType::SrvBuffer { .. } => D3D12_RESOURCE_STATE_COMMON,
            DescriptorType::Srv2DImage { .. } => D3D12_RESOURCE_STATE_COMMON,
            DescriptorType::SrvCubemap { .. } => D3D12_RESOURCE_STATE_COMMON,
            DescriptorType::Srv3DImage { .. } => D3D12_RESOURCE_STATE_COMMON,
            DescriptorType::Rtv { .. } => D3D12_RESOURCE_STATE_RENDER_TARGET,
            DescriptorType::Dsv { .. } => D3D12_RESOURCE_STATE_DEPTH_WRITE,
        }
    }

    fn flags(&self) -> D3D12_RESOURCE_FLAGS {
        match *self {
            DescriptorType::SrvBuffer { .. } => D3D12_RESOURCE_FLAG_NONE,
            DescriptorType::Srv2DImage { .. } => D3D12_RESOURCE_FLAG_NONE,
            DescriptorType::SrvCubemap { .. } => D3D12_RESOURCE_FLAG_NONE,
            DescriptorType::Srv3DImage { .. } => D3D12_RESOURCE_FLAG_NONE,
            DescriptorType::Rtv { .. } => D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
            DescriptorType::Dsv { .. } => {
                D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL | D3D12_RESOURCE_FLAG_DENY_SHADER_RESOURCE
            }
        }
    }

    fn optimized_clear_value(&self) -> Option<D3D12_CLEAR_VALUE> {
        match *self {
            DescriptorType::SrvBuffer { .. } => None,
            DescriptorType::Srv2DImage { .. } => None,
            DescriptorType::SrvCubemap { .. } => None,
            DescriptorType::Srv3DImage { .. } => None,
            DescriptorType::Rtv { format } => Some(D3D12_CLEAR_VALUE {
                Format: format,
                Anonymous: D3D12_CLEAR_VALUE_0 { Color: [0.0, 0.0, 0.0, 1.0] },
            }),
            DescriptorType::Dsv { format } => Some(D3D12_CLEAR_VALUE {
                Format: format,
                Anonymous: D3D12_CLEAR_VALUE_0 {
                    DepthStencil: D3D12_DEPTH_STENCIL_VALUE { Depth: 0.0, Stencil: 0 },
                },
            }),
        }
    }
}

#[derive(Clone)]
pub struct Descriptor {
    pub handle: D3D12_CPU_DESCRIPTOR_HANDLE,
    pub index: u32,
}

pub struct DescriptorHeap {
    heap: ID3D12DescriptorHeap,
    base_handle: D3D12_CPU_DESCRIPTOR_HANDLE,
    gpu_base_handle: Option<D3D12_GPU_DESCRIPTOR_HANDLE>,
    handle_size: u32,
    free_list: Vec<u32>,
    next_index: u32,
}

impl DescriptorHeap {
    pub fn new(device: &ID3D12Device, ty: D3D12_DESCRIPTOR_HEAP_TYPE) -> anyhow::Result<Self> {
        unsafe {
            let flags = match ty {
                D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV | D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER => {
                    D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE
                }
                _ => D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
            };
            let num = match ty {
                D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV => RESOURCE_HEAP_SIZE,
                D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER => SAMPLER_HEAP_SIZE,
                D3D12_DESCRIPTOR_HEAP_TYPE_RTV => MAX_RENDER_TARGETS,
                D3D12_DESCRIPTOR_HEAP_TYPE_DSV => MAX_DEPTH_TARGETS,
                _ => unreachable!(),
            };
            let name = match ty {
                D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV => "Shader Resource Heap",
                D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER => "Sampler Heap",
                D3D12_DESCRIPTOR_HEAP_TYPE_RTV => "Render Target Heap",
                D3D12_DESCRIPTOR_HEAP_TYPE_DSV => "Depth Stencil Heap",
                _ => unreachable!(),
            };
            let heap_desc = D3D12_DESCRIPTOR_HEAP_DESC {
                Type: ty,
                NumDescriptors: num,
                Flags: flags,
                NodeMask: 0,
            };

            let heap: ID3D12DescriptorHeap = device
                .CreateDescriptorHeap(&heap_desc)
                .context("Failed to create descriptor heap")?;
            heap.set_name(name);

            let base_handle = heap.GetCPUDescriptorHandleForHeapStart();
            let gpu_base_handle = if flags.contains(D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE) {
                Some(heap.GetGPUDescriptorHandleForHeapStart())
            } else {
                None
            };
            let handle_size = device.GetDescriptorHandleIncrementSize(ty);

            Ok(Self {
                heap,
                base_handle,
                gpu_base_handle,
                handle_size,
                free_list: Vec::new(),
                next_index: 0,
            })
        }
    }

    pub fn allocate(
        &mut self,
        device: &ID3D12Device,
        resource: &ID3D12Resource,
        ty: DescriptorType,
    ) -> Descriptor {
        let index = if let Some(index) = self.free_list.pop() {
            index
        } else {
            let index = self.next_index;
            self.next_index += 1;
            index
        };

        let handle = D3D12_CPU_DESCRIPTOR_HANDLE {
            ptr: self.base_handle.ptr + index as usize * self.handle_size as usize,
        };

        Self::allocate_inner(device, resource, ty, handle);

        Descriptor { handle, index }
    }

    pub fn rewrite(
        &self,
        device: &ID3D12Device,
        resource: &ID3D12Resource,
        ty: DescriptorType,
        descriptor: &Descriptor,
    ) {
        Self::allocate_inner(device, resource, ty, descriptor.handle);
    }

    fn allocate_inner(
        device: &ID3D12Device,
        resource: &ID3D12Resource,
        ty: DescriptorType,
        handle: D3D12_CPU_DESCRIPTOR_HANDLE,
    ) {
        match ty {
            DescriptorType::SrvBuffer { offset, count, mut stride, raw } => {
                let format;
                let flags;

                if raw {
                    format = DXGI_FORMAT_R32_TYPELESS;
                    stride = 0;
                    flags = D3D12_BUFFER_SRV_FLAG_RAW;
                } else {
                    format = DXGI_FORMAT_UNKNOWN;
                    flags = D3D12_BUFFER_SRV_FLAG_NONE;
                }

                let srv_desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
                    Format: format,
                    ViewDimension: D3D12_SRV_DIMENSION_BUFFER,
                    Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                    Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                        Buffer: D3D12_BUFFER_SRV {
                            FirstElement: offset,
                            NumElements: count,
                            StructureByteStride: stride,
                            Flags: flags,
                        },
                    },
                };

                unsafe {
                    device.CreateShaderResourceView(Some(resource), Some(&srv_desc), handle);
                }
            }
            DescriptorType::Srv2DImage { format, mipmaps } => {
                let srv_desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
                    Format: format,
                    ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                    Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                    Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                        Texture2D: D3D12_TEX2D_SRV {
                            MostDetailedMip: 0,
                            MipLevels: mipmaps as _,
                            PlaneSlice: 0,
                            ResourceMinLODClamp: 0.0,
                        },
                    },
                };

                unsafe {
                    device.CreateShaderResourceView(Some(resource), Some(&srv_desc), handle);
                }
            }
            DescriptorType::SrvCubemap { format, mipmaps } => {
                let srv_desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
                    Format: format,
                    ViewDimension: D3D12_SRV_DIMENSION_TEXTURECUBE,
                    Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                    Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                        TextureCube: D3D12_TEXCUBE_SRV {
                            MostDetailedMip: 0,
                            MipLevels: mipmaps as _,
                            ResourceMinLODClamp: 0.0,
                        },
                    },
                };

                unsafe {
                    device.CreateShaderResourceView(Some(resource), Some(&srv_desc), handle);
                }
            }
            DescriptorType::Srv3DImage { format, .. } => {
                let srv_desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
                    Format: format,
                    ViewDimension: D3D12_SRV_DIMENSION_TEXTURE3D,
                    Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                    Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                        Texture3D: D3D12_TEX3D_SRV {
                            MostDetailedMip: 0,
                            MipLevels: 1,
                            ResourceMinLODClamp: 0.0,
                        },
                    },
                };

                unsafe {
                    device.CreateShaderResourceView(Some(resource), Some(&srv_desc), handle);
                }
            }
            DescriptorType::Rtv { format } => {
                let rtv_desc = D3D12_RENDER_TARGET_VIEW_DESC {
                    Format: format,
                    ViewDimension: D3D12_RTV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D12_RENDER_TARGET_VIEW_DESC_0 {
                        Texture2D: D3D12_TEX2D_RTV { MipSlice: 0, PlaneSlice: 0 },
                    },
                };

                unsafe {
                    device.CreateRenderTargetView(Some(resource), Some(&rtv_desc), handle);
                }
            }
            DescriptorType::Dsv { format } => {
                let dsv_desc = D3D12_DEPTH_STENCIL_VIEW_DESC {
                    Format: format,
                    ViewDimension: D3D12_DSV_DIMENSION_TEXTURE2D,
                    Flags: D3D12_DSV_FLAG_NONE,
                    Anonymous: D3D12_DEPTH_STENCIL_VIEW_DESC_0 {
                        Texture2D: D3D12_TEX2D_DSV { MipSlice: 0 },
                    },
                };

                unsafe {
                    device.CreateDepthStencilView(Some(resource), Some(&dsv_desc), handle);
                }
            }
        }
    }

    pub fn heap(&self) -> ID3D12DescriptorHeap {
        self.heap.clone()
    }

    pub fn base_gpu_handle(&self) -> D3D12_GPU_DESCRIPTOR_HANDLE {
        self.gpu_base_handle.unwrap()
    }

    pub fn gpu_handle(&self, pointer: TexturePointer) -> D3D12_GPU_DESCRIPTOR_HANDLE {
        D3D12_GPU_DESCRIPTOR_HANDLE {
            ptr: self.gpu_base_handle.unwrap().ptr + pointer.0 as u64 * self.handle_size as u64,
        }
    }
}

pub struct Buffer {
    allocator: Arc<Mutex<Allocator>>,
    allocation: Option<Allocation>,
    resource: ID3D12Resource,
    // SAFETY: This isn't static
    mapping: Option<&'static mut [u8]>,
    // Requested size, not allocation size
    size: u64,
}

impl Buffer {
    pub fn new(
        device: &ID3D12Device,
        allocator: &Arc<Mutex<Allocator>>,
        location: MemoryLocation,
        size: u64,
        uav: bool,
        label: &str,
    ) -> anyhow::Result<Self> {
        let flags =
            if uav { D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS } else { D3D12_RESOURCE_FLAG_NONE };

        let resource_desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Alignment: 0,
            Width: size,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_UNKNOWN,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            Flags: flags,
        };

        let allocator = Arc::clone(allocator);

        let allocation = allocator
            .lock()
            .allocate(&AllocationCreateDesc::from_d3d12_resource_desc(
                device,
                &resource_desc,
                label,
                location,
            ))
            .context("Allocation Failure")?;

        let resource = unsafe {
            let mut resource: Option<ID3D12Resource> = None;

            device
                .CreatePlacedResource(
                    allocation.heap(),
                    allocation.offset(),
                    &resource_desc,
                    D3D12_RESOURCE_STATE_COMMON,
                    None,
                    &mut resource,
                )
                .context("Failed to create placed resource")?;

            resource.unwrap()
        };
        resource.set_name(label);

        let mappable = matches!(location, MemoryLocation::GpuToCpu | MemoryLocation::CpuToGpu);

        let mapping = if mappable {
            Some(unsafe {
                let mut data = std::ptr::null_mut();
                resource.Map(0, None, Some(&mut data)).context("Failed to map resource")?;
                std::slice::from_raw_parts_mut(data as *mut u8, size as usize)
            })
        } else {
            None
        };

        Ok(Self { allocator, allocation: Some(allocation), resource, mapping, size })
    }

    pub fn create_descriptor(
        &self,
        device: &ID3D12Device,
        descriptor_pool: &mut DescriptorHeap,
        stride: u32,
    ) -> u32 {
        descriptor_pool
            .allocate(
                device,
                &self.resource,
                DescriptorType::SrvBuffer {
                    offset: 0,
                    count: self.size as u32 / stride,
                    stride,
                    raw: false,
                },
            )
            .index
    }

    pub fn resource(&self) -> &ID3D12Resource {
        &self.resource
    }

    pub fn gpu_address(&self) -> u64 {
        unsafe { self.resource.GetGPUVirtualAddress() }
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn mapping(&mut self) -> &mut [u8] {
        self.mapping.as_deref_mut().unwrap()
    }

    pub fn frame_range(&self, index: u32) -> std::ops::Range<usize> {
        let size = self.size as usize;
        let half_size = size / 2;
        let offset = index as usize * half_size;
        offset..offset + half_size
    }

    pub fn frame_mapping(&mut self, index: u32) -> &mut [u8] {
        let range = self.frame_range(index);
        let mapping = self.mapping();

        &mut mapping[range]
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        if self.mapping.is_some() {
            unsafe {
                self.resource.Unmap(0, None);
            }
        }
        self.allocator
            .lock()
            .free(self.allocation.take().unwrap())
            .expect("Failed to free allocation");
    }
}

#[allow(unused)]
pub enum BufferViewType {
    Raw,
    Typed { stride: u32 },
}

impl BufferViewType {
    pub fn stride(&self) -> u32 {
        match *self {
            BufferViewType::Raw => 4,
            BufferViewType::Typed { stride } => stride,
        }
    }

    pub fn is_raw(&self) -> bool {
        matches!(self, BufferViewType::Raw)
    }
}

pub struct OverwritingBufferBelt {
    gpu_buffer: Buffer,
    cpu_buffer: Buffer,
    size: u64,
}

impl OverwritingBufferBelt {
    pub fn new(
        device: &ID3D12Device,
        allocator: &Arc<Mutex<Allocator>>,
        size: u64,
        label: &str,
    ) -> anyhow::Result<Self> {
        let rounded_size =
            size.next_multiple_of(D3D12_CONSTANT_BUFFER_DATA_PLACEMENT_ALIGNMENT as u64);
        let buffer_size = rounded_size * 2;

        let gpu_buffer = Buffer::new(
            device,
            allocator,
            MemoryLocation::GpuOnly,
            buffer_size,
            false,
            &format!("{} GPU", label),
        )?;

        let cpu_buffer = Buffer::new(
            device,
            allocator,
            MemoryLocation::CpuToGpu,
            buffer_size,
            false,
            &format!("{} CPU", label),
        )?;

        Ok(Self { gpu_buffer, cpu_buffer, size })
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn create_descriptors(
        &mut self,
        device: &ID3D12Device,
        descriptor_heap: &mut DescriptorHeap,
        ty: BufferViewType,
    ) -> [u32; FRAMES_IN_FLIGHT] {
        let gpu_range_0 = self.gpu_buffer.frame_range(0);

        let stride = ty.stride();
        let raw = ty.is_raw();

        let srv_index_0 = descriptor_heap
            .allocate(
                device,
                self.gpu_buffer.resource(),
                DescriptorType::SrvBuffer {
                    offset: gpu_range_0.start as u64 / stride as u64,
                    count: (gpu_range_0.end - gpu_range_0.start) as u32 / stride,
                    stride,
                    raw,
                },
            )
            .index;

        let gpu_range_1 = self.gpu_buffer.frame_range(1);

        let desc_type1 = DescriptorType::SrvBuffer {
            offset: gpu_range_1.start as u64 / stride as u64,
            count: (gpu_range_1.end - gpu_range_1.start) as u32 / stride,
            stride,
            raw,
        };

        let srv_index_1 =
            descriptor_heap.allocate(device, self.gpu_buffer.resource(), desc_type1).index;

        [srv_index_0, srv_index_1]
    }

    pub fn upload_data(
        &mut self,
        index: FrameIndex,
        copy_command_list: &ID3D12GraphicsCommandList,
        data: &[u8],
    ) {
        let cpu_index = index.get_cpu_index();
        let cpu_mapping = self.cpu_buffer.frame_mapping(cpu_index);

        // Copy data to CPU buffer
        cpu_mapping[..data.len()].copy_from_slice(data);

        // Encode copy from CPU to GPU buffer
        let copy_queue_index = index.get_gpu_index();
        let copy_queue_range = self.cpu_buffer.frame_range(copy_queue_index);

        unsafe {
            copy_command_list.CopyBufferRegion(
                self.gpu_buffer.resource(),
                copy_queue_range.start as u64,
                self.cpu_buffer.resource(),
                copy_queue_range.start as u64,
                data.len() as u64,
            );
        }
    }

    pub fn gpu_address(&self, index: FrameIndex) -> u64 {
        let frame_range = self.gpu_buffer.frame_range(index.get_gpu_index());
        self.gpu_buffer.gpu_address() + frame_range.start as u64
    }
}

pub struct Texture {
    allocator: Arc<Mutex<Allocator>>,
    allocation: Option<Allocation>,
    resource: ID3D12Resource,
    srv_descriptor: Option<Descriptor>,
    rtv_descriptor: Option<Descriptor>,
    dsv_descriptor: Option<Descriptor>,
    ty: DescriptorType,
    size: UVec2,
}

impl Texture {
    pub fn new(
        device: &ID3D12Device,
        allocator: &Arc<Mutex<Allocator>>,
        descriptor_ty: DescriptorType,
        size: UVec2,
        label: &str,
    ) -> anyhow::Result<Self> {
        let format = descriptor_ty.format();
        let mipmaps = descriptor_ty.mipmaps();
        let depth_or_array_layers = descriptor_ty.depth_or_array_layers();
        let resource_desc = D3D12_RESOURCE_DESC {
            Dimension: descriptor_ty.dimension(),
            Alignment: 0,
            Width: size.x as u64,
            Height: size.y,
            DepthOrArraySize: depth_or_array_layers,
            MipLevels: mipmaps,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: descriptor_ty.flags(),
        };

        let allocator = Arc::clone(allocator);

        let allocation = allocator
            .lock()
            .allocate(&AllocationCreateDesc::from_d3d12_resource_desc(
                device,
                &resource_desc,
                label,
                MemoryLocation::GpuOnly,
            ))
            .context("Allocation Failure")?;

        let resource = unsafe {
            let mut resource: Option<ID3D12Resource> = None;

            let clear_value = descriptor_ty.optimized_clear_value();
            device
                .CreatePlacedResource(
                    allocation.heap(),
                    allocation.offset(),
                    &resource_desc,
                    descriptor_ty.initial_state(),
                    clear_value.as_ref().map(|r| r as *const _),
                    &mut resource,
                )
                .context("Failed to create placed resource")?;

            resource.unwrap()
        };
        resource.set_name(label);

        Ok(Self {
            allocator,
            allocation: Some(allocation),
            resource,
            srv_descriptor: None,
            rtv_descriptor: None,
            dsv_descriptor: None,
            size,
            ty: descriptor_ty,
        })
    }

    pub fn create_descriptor(
        &mut self,
        device: &ID3D12Device,
        descriptor_pool: &mut DescriptorHeap,
        descriptor_ty: DescriptorType,
        input_descriptor: Option<&Descriptor>,
    ) {
        let descriptor = if let Some(descriptor) = input_descriptor {
            descriptor_pool.rewrite(device, &self.resource, descriptor_ty, descriptor);
            descriptor.clone()
        } else {
            descriptor_pool.allocate(device, &self.resource, descriptor_ty)
        };

        match descriptor_ty {
            DescriptorType::Srv2DImage { .. }
            | DescriptorType::SrvCubemap { .. }
            | DescriptorType::Srv3DImage { .. } => self.srv_descriptor = Some(descriptor.clone()),
            DescriptorType::Rtv { .. } => self.rtv_descriptor = Some(descriptor.clone()),
            DescriptorType::Dsv { .. } => self.dsv_descriptor = Some(descriptor.clone()),
            DescriptorType::SrvBuffer { .. } => {
                unreachable!()
            }
        }
    }

    pub fn srv_descriptor(&self) -> &Descriptor {
        self.srv_descriptor.as_ref().unwrap()
    }

    pub fn rtv_descriptor(&self) -> &Descriptor {
        self.rtv_descriptor.as_ref().unwrap()
    }

    pub fn dsv_descriptor(&self) -> &Descriptor {
        self.dsv_descriptor.as_ref().unwrap()
    }

    pub fn resource(&self) -> &ID3D12Resource {
        &self.resource
    }
}

impl Drop for Texture {
    fn drop(&mut self) {
        self.allocator
            .lock()
            .free(self.allocation.take().unwrap())
            .expect("Failed to free allocation");
    }
}

pub struct StagingBuffer {
    buffer: Buffer,
    used_size: u64,
}

impl StagingBuffer {
    pub fn new(
        device: &ID3D12Device,
        allocator: &Arc<Mutex<Allocator>>,
        size: u64,
        label: &str,
    ) -> anyhow::Result<Self> {
        let buffer = Buffer::new(device, allocator, MemoryLocation::CpuToGpu, size, false, label)?;

        Ok(Self { buffer, used_size: 0 })
    }

    pub fn upload_mesh(
        &mut self,
        command_list: &ID3D12GraphicsCommandList,
        target_buffer: &ID3D12Resource,
        pending: &PendingMeshData,
    ) {
        let vertex_size = pending.descriptor.vertex_data.len() * mem::size_of::<VertexData>();
        let index_size = pending.descriptor.index_data.len() * mem::size_of::<u32>();

        let vertex_start = self.used_size;
        let index_start = vertex_start + vertex_size as u64;
        let index_end = index_start + index_size as u64;

        let total_bytes = index_end - vertex_start;

        let vertex_mapping =
            &mut self.buffer.mapping()[vertex_start as usize..index_start as usize];
        vertex_mapping.copy_from_slice(bytemuck::cast_slice(&pending.descriptor.vertex_data));

        let index_mapping = &mut self.buffer.mapping()[index_start as usize..index_end as usize];
        index_mapping.copy_from_slice(bytemuck::cast_slice(&pending.descriptor.index_data));

        unsafe {
            command_list.CopyBufferRegion(
                target_buffer,
                pending.destination_offset,
                self.buffer.resource(),
                vertex_start,
                total_bytes,
            );
        }

        self.used_size = index_end;
    }

    pub fn upload_to_texture(
        &mut self,
        texture: &Texture,
        command_list: &ID3D12GraphicsCommandList,
        region: Option<TextureUploadRegion>,
        data: &[u8],
    ) {
        const COPY_ALIGNMENT: u32 = D3D12_TEXTURE_DATA_PITCH_ALIGNMENT;

        let block_size = match texture.ty.format() {
            DXGI_FORMAT_R8G8B8A8_UNORM_SRGB => 1,
            DXGI_FORMAT_R9G9B9E5_SHAREDEXP => 1,
            DXGI_FORMAT_BC1_UNORM_SRGB => 4,
            DXGI_FORMAT_BC5_UNORM => 4,
            DXGI_FORMAT_BC6H_UF16 => 4,
            DXGI_FORMAT_BC7_UNORM_SRGB => 4,
            _ => unimplemented!(),
        };

        let bytes_per_block = match texture.ty.format() {
            DXGI_FORMAT_R8G8B8A8_UNORM_SRGB => 4,
            DXGI_FORMAT_R9G9B9E5_SHAREDEXP => 4,
            DXGI_FORMAT_BC1_UNORM_SRGB => 8,
            DXGI_FORMAT_BC5_UNORM => 16,
            DXGI_FORMAT_BC6H_UF16 => 16,
            DXGI_FORMAT_BC7_UNORM_SRGB => 16,
            _ => unimplemented!(),
        };

        // If we are given a box, we only upload that region.

        let top_left;
        let mipmaps;
        let size;
        let depth_or_array_layers;
        let depth;

        if let Some(region) = region {
            top_left = region.top_left;
            mipmaps = 1;
            size = region.size;
            depth_or_array_layers = 1;
            depth = 1;

            assert_eq!(texture.ty.mipmaps(), 1)
        } else {
            top_left = UVec2::ZERO;
            mipmaps = texture.ty.mipmaps();
            size = texture.size;
            depth_or_array_layers = texture.ty.depth_or_array_layers();
            depth = texture.ty.depth();
        }

        let mut space_required = 0;
        for mipmap in 0..mipmaps {
            let mip_size = (size >> mipmap).max(UVec2::splat(1));
            let block_count =
                UVec2::new(mip_size.x.div_ceil(block_size), mip_size.y.div_ceil(block_size));
            let row_pitch =
                block_count.x.next_multiple_of(COPY_ALIGNMENT / bytes_per_block) * bytes_per_block;

            space_required += row_pitch * block_count.y * depth_or_array_layers as u32;
        }
        let start_offset = self.used_size as usize;

        let mut current_data_position = 0_usize;
        let mut current_buffer_position = start_offset;

        assert!(
            start_offset + space_required as usize <= self.buffer.size() as usize,
            "Not enough space in staging buffer for texture upload: {} > {}",
            start_offset + space_required as usize,
            self.buffer.size() as usize
        );

        let array_layers = texture.ty.array_layers();

        for mipmap in 0..mipmaps {
            let mip_size = (size >> mipmap).max(UVec2::splat(1));
            let block_count =
                UVec2::new(mip_size.x.div_ceil(block_size), mip_size.y.div_ceil(block_size));
            let data_row_pitch = block_count.x * bytes_per_block;
            let buffer_row_pitch =
                block_count.x.next_multiple_of(COPY_ALIGNMENT / bytes_per_block) * bytes_per_block;

            let subresource = D3D12_SUBRESOURCE_FOOTPRINT {
                Format: texture.ty.format(),
                Width: mip_size.x,
                Height: mip_size.y,
                Depth: depth as u32,
                RowPitch: buffer_row_pitch,
            };

            for array_layer in 0..array_layers {
                let dst = D3D12_TEXTURE_COPY_LOCATION {
                    pResource: texture.resource.to_option_unowned(),
                    Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                    Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                        // Subresource index goes through all mipmaps of each array layer.
                        SubresourceIndex: array_layer as u32 * mipmaps as u32 + mipmap as u32,
                    },
                };

                let src = D3D12_TEXTURE_COPY_LOCATION {
                    pResource: self.buffer.resource().to_option_unowned(),
                    Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                    Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                        PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                            Offset: current_buffer_position as u64,
                            Footprint: subresource,
                        },
                    },
                };

                for _ in 0..depth {
                    for _ in 0..(mip_size.y.div_ceil(block_size)) {
                        let data_offset = current_data_position;
                        let buffer_offset = current_buffer_position;

                        let mapping = self.buffer.mapping();
                        let buffer_slice =
                            &mut mapping[buffer_offset..buffer_offset + data_row_pitch as usize];
                        let data_slice = &data[data_offset..data_offset + data_row_pitch as usize];
                        buffer_slice.copy_from_slice(data_slice);

                        current_data_position += data_row_pitch as usize;
                        current_buffer_position += buffer_row_pitch as usize;
                    }
                }

                unsafe {
                    command_list.CopyTextureRegion(&dst, top_left.x, top_left.y, 0, &src, None);
                }
            }
        }

        self.used_size += space_required as u64;
    }

    pub fn clear(&mut self) {
        self.used_size = 0;
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TextureUploadRegion {
    pub top_left: UVec2,
    pub size: UVec2,
}
