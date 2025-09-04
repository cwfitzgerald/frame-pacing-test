use std::{
    any::Any,
    collections::{hash_map::Entry, HashMap},
    ffi::c_void,
    sync::Arc,
};

use anyhow::Context;
use glam::UVec2;
use gpu_allocator::d3d12::Allocator;
use parking_lot::Mutex;
use windows::Win32::{
    Foundation::{self, TRUE},
    Graphics::{Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, Direct3D12::*, Dxgi::Common::*},
};

use crate::{
    renderer::{create_root_signature, FrameIndex},
    resource::{DescriptorHeap, OverwritingBufferBelt, StagingBuffer, Texture, RESOURCE_HEAP_SIZE},
    util::{ID3D12ObjectExt, InterfaceExt},
};

// 128 KiB
const VERTEX_DATA_SIZE: u64 = 1024 * 128;

pub struct EguiRendererInput {
    /// Texture changes since last frame (including the font texture).
    ///
    /// The backend needs to apply [`crate::TexturesDelta::set`] _before_ painting,
    /// and free any texture in [`crate::TexturesDelta::free`] _after_ painting.
    ///
    /// It is assumed that all egui viewports share the same painter and texture namespace.
    pub textures_delta: egui::epaint::textures::TexturesDelta,

    /// What to paint.
    ///
    /// You can use [`crate::Context::tessellate`] to turn this into triangles.
    pub primitives: Vec<egui::ClippedPrimitive>,
}

struct EguiMesh {
    clip_rect: egui::emath::Rect,
    texture_id: egui::TextureId,
    vertex_offset: u32,
    index_offset: u32,
    index_count: u32,
}

pub struct EguiRenderer {
    texture_cache: HashMap<egui::TextureId, Texture>,
    vertex_buffer: OverwritingBufferBelt,
    meshes: Vec<EguiMesh>,
    data_scratch: Vec<u8>,

    root_signature: ID3D12RootSignature,
    pipeline_state: ID3D12PipelineState,
}

impl EguiRenderer {
    pub fn new(device: &ID3D12Device, allocator: &Arc<Mutex<Allocator>>) -> anyhow::Result<Self> {
        let vertex_buffer = OverwritingBufferBelt::new(
            device,
            allocator,
            VERTEX_DATA_SIZE,
            "Egui Vertex/Index Buffer",
        )?;

        let ranges = [
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
                        Num32BitValues: 4,
                    },
                },
            },
            D3D12_ROOT_PARAMETER1 {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_SRV,
                ShaderVisibility: D3D12_SHADER_VISIBILITY_VERTEX,
                Anonymous: D3D12_ROOT_PARAMETER1_0 {
                    Descriptor: D3D12_ROOT_DESCRIPTOR1 {
                        ShaderRegister: 0,
                        RegisterSpace: 0,
                        Flags: D3D12_ROOT_DESCRIPTOR_FLAG_DATA_STATIC_WHILE_SET_AT_EXECUTE,
                    },
                },
            },
            D3D12_ROOT_PARAMETER1 {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
                ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
                Anonymous: D3D12_ROOT_PARAMETER1_0 {
                    DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE1 {
                        NumDescriptorRanges: ranges.len() as u32,
                        pDescriptorRanges: ranges.as_ptr(),
                    },
                },
            },
        ];

        let samplers = [D3D12_STATIC_SAMPLER_DESC {
            Filter: D3D12_FILTER_MIN_MAG_MIP_LINEAR,
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
        }];

        let root_signature = create_root_signature(device, &parameters, &samplers)
            .context("Failed to create root signature")?;

        let vs_dxil = include_bytes!("../../shaders/dxil/egui.vs.cso");
        let ps_dxil = include_bytes!("../../shaders/dxil/egui.ps.cso");

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
                blend_state.RenderTarget[0] = D3D12_RENDER_TARGET_BLEND_DESC {
                    BlendEnable: TRUE,
                    SrcBlend: D3D12_BLEND_SRC_ALPHA,
                    DestBlend: D3D12_BLEND_INV_SRC_ALPHA,
                    BlendOp: D3D12_BLEND_OP_ADD,
                    SrcBlendAlpha: D3D12_BLEND_ONE,
                    DestBlendAlpha: D3D12_BLEND_ZERO,
                    BlendOpAlpha: D3D12_BLEND_OP_ADD,
                    RenderTargetWriteMask: D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8,
                    ..Default::default()
                };
                blend_state
            },
            SampleMask: u32::MAX,
            RasterizerState: D3D12_RASTERIZER_DESC {
                FillMode: D3D12_FILL_MODE_SOLID,
                CullMode: D3D12_CULL_MODE_NONE,
                DepthClipEnable: TRUE,
                ..Default::default()
            },
            PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
            NumRenderTargets: 1,
            RTVFormats: {
                let mut formats = [DXGI_FORMAT::default(); 8];
                formats[0] = DXGI_FORMAT_R8G8B8A8_UNORM_SRGB;
                formats
            },
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, ..Default::default() },
            ..Default::default()
        };

        let pipeline_state: ID3D12PipelineState = unsafe {
            device
                .CreateGraphicsPipelineState(&pipeline_desc)
                .context("Failed to create pipeline state")?
        };
        pipeline_state.set_name("Egui");

        Ok(Self {
            texture_cache: HashMap::new(),
            vertex_buffer,
            meshes: Vec::new(),
            data_scratch: Vec::with_capacity(VERTEX_DATA_SIZE as usize),

            root_signature,
            pipeline_state,
        })
    }

    pub fn upload(
        &mut self,
        device: &ID3D12Device,
        allocator: &Arc<Mutex<Allocator>>,
        graphics_command_list: &ID3D12GraphicsCommandList,
        copy_command_list: &ID3D12GraphicsCommandList,
        descriptor_heap: &mut DescriptorHeap,
        frame_index: FrameIndex,
        staging_buffer: &mut StagingBuffer,
        to_destroy: &mut Vec<Box<dyn Any>>,
        input: &EguiRendererInput,
    ) {
        let mut pre_barriers = Vec::new();
        let mut post_barriers = Vec::new();

        // First iterate over all textures that will be updated, and transition them to COPY_DEST.
        for (id, delta) in &input.textures_delta.set {
            if delta.is_whole() {
                continue;
            }

            if let Some(texture) = self.texture_cache.get(&id) {
                pre_barriers.push(crate::util::transition_barrier(
                    texture.resource(),
                    D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                ));

                post_barriers.push(crate::util::transition_barrier(
                    texture.resource(),
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
                ));
            }
        }

        if !pre_barriers.is_empty() {
            unsafe {
                graphics_command_list.ResourceBarrier(&pre_barriers);
            }
        }

        for (id, delta) in &input.textures_delta.set {
            log::info!("Applying delta for texture {id:?} whole: {}", delta.is_whole());

            let entry = self.texture_cache.entry(*id);
            // If we're uploading the whole image, we always make a new texture,
            // that way we can utilize the copy-queue for the upload.
            //
            // For partial updates, we need to do it on the graphics queue.
            let needs_new_texture = delta.is_whole();

            let texture = if needs_new_texture {
                let size = delta.image.size();
                let mut new_texture = Texture::new(
                    device,
                    allocator,
                    crate::resource::DescriptorType::Srv2DImage {
                        format: DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
                        mipmaps: 1,
                    },
                    UVec2::new(size[0] as u32, size[1] as u32),
                    &format!("Egui Texture {:?}", id),
                )
                .unwrap();

                new_texture.create_descriptor(
                    &device,
                    descriptor_heap,
                    crate::resource::DescriptorType::Srv2DImage {
                        format: DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
                        mipmaps: 1,
                    },
                    None,
                );

                match entry {
                    Entry::Occupied(mut entry) => {
                        let old_texture = entry.insert(new_texture);
                        to_destroy.push(Box::new(old_texture));
                        entry.into_mut()
                    }
                    Entry::Vacant(entry) => entry.insert(new_texture),
                }
            } else {
                let Entry::Occupied(entry) = entry else { unimplemented!() };

                entry.into_mut()
            };

            let region = if let Some(top_left) = delta.pos {
                let top_left = UVec2::new(top_left[0] as u32, top_left[1] as u32);
                let size = delta.image.size();
                let size = UVec2::new(size[0] as u32, size[1] as u32);

                Some(crate::resource::TextureUploadRegion { top_left, size })
            } else {
                None
            };

            let font_pixels: Vec<_>;
            let data = match delta.image {
                egui::epaint::ImageData::Color(ref color) => color.as_raw(),
                egui::epaint::ImageData::Font(ref font) => {
                    font_pixels = font.srgba_pixels(None).collect();
                    bytemuck::cast_slice(&font_pixels)
                }
            };

            let command_list =
                if needs_new_texture { copy_command_list } else { graphics_command_list };

            staging_buffer.upload_to_texture(texture, command_list, region, data);
        }

        if !post_barriers.is_empty() {
            unsafe {
                graphics_command_list.ResourceBarrier(&post_barriers);
            }
        }

        self.upload_buffers(frame_index, copy_command_list, &input.primitives);
    }

    fn upload_buffers(
        &mut self,
        index: FrameIndex,
        copy_command_list: &ID3D12GraphicsCommandList,
        clipped_primitives: &[egui::ClippedPrimitive],
    ) {
        self.data_scratch.clear();
        self.meshes.clear();

        for primitive in clipped_primitives {
            let egui::epaint::Primitive::Mesh(ref mesh) = primitive.primitive else {
                unreachable!()
            };

            let vertex_offset = self.data_scratch.len() as u32;
            self.data_scratch.extend_from_slice(bytemuck::cast_slice(&mesh.vertices));
            let index_offset = self.data_scratch.len() as u32;
            self.data_scratch.extend_from_slice(bytemuck::cast_slice(&mesh.indices));

            self.meshes.push(EguiMesh {
                clip_rect: primitive.clip_rect,
                texture_id: mesh.texture_id,
                vertex_offset,
                index_offset,
                index_count: mesh.indices.len() as u32,
            });
        }

        self.vertex_buffer.upload_data(index, copy_command_list, &self.data_scratch);
    }

    pub fn render(
        &mut self,
        graphics_command_list: &ID3D12GraphicsCommandList,
        descriptor_heap: &DescriptorHeap,
        frame_index: FrameIndex,
        screen_size: UVec2,
    ) {
        unsafe {
            graphics_command_list.SetGraphicsRootSignature(&self.root_signature);
            graphics_command_list
                .SetGraphicsRootShaderResourceView(1, self.vertex_buffer.gpu_address(frame_index));
            graphics_command_list
                .SetGraphicsRootDescriptorTable(2, descriptor_heap.base_gpu_handle());
            graphics_command_list.SetPipelineState(&self.pipeline_state);
            graphics_command_list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);

            for mesh in self.meshes.drain(..) {
                let registers = Registers {
                    screen_size,
                    vertex_offset: mesh.vertex_offset,
                    texture_id: self.texture_cache[&mesh.texture_id].srv_descriptor().index,
                };

                graphics_command_list.SetGraphicsRoot32BitConstants(
                    0,
                    4,
                    bytemuck::bytes_of(&registers) as *const _ as *const _,
                    0,
                );
                graphics_command_list.RSSetScissorRects(&[Foundation::RECT {
                    left: mesh.clip_rect.min.x as i32,
                    top: mesh.clip_rect.min.y as i32,
                    right: mesh.clip_rect.max.x as i32,
                    bottom: mesh.clip_rect.max.y as i32,
                }]);
                graphics_command_list.IASetIndexBuffer(Some(&D3D12_INDEX_BUFFER_VIEW {
                    BufferLocation: self.vertex_buffer.gpu_address(frame_index),
                    SizeInBytes: self.vertex_buffer.size() as u32,
                    Format: DXGI_FORMAT_R32_UINT,
                }));
                graphics_command_list.DrawIndexedInstanced(
                    mesh.index_count,
                    1,
                    mesh.index_offset / 4,
                    0,
                    0,
                );
            }
        }
    }

    pub fn cleanup(&mut self, input: &EguiRendererInput, to_destroy: &mut Vec<Box<dyn Any>>) {
        for id in &input.textures_delta.free {
            if let Some(texture) = self.texture_cache.remove(&id) {
                to_destroy.push(Box::new(texture));
            }
        }
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Registers {
    screen_size: UVec2,
    vertex_offset: u32,
    texture_id: u32,
}
