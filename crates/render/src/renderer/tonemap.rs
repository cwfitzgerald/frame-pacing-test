use std::{ffi::c_void, io};

use anyhow::Context;
use common::assets::Asset;
use glam::UVec2;
use windows::Win32::Graphics::{Direct3D::*, Direct3D12::*, Dxgi::Common::*};

use crate::{
    renderer::{create_root_signature, TextureUploadContext},
    resource::{DescriptorHeap, Texture, RESOURCE_HEAP_SIZE},
    util::{ID3D12ObjectExt, InterfaceExt},
    Renderer,
};

pub struct Tonemapper {
    root_signature: ID3D12RootSignature,
    pipeline_state: ID3D12PipelineState,
}

impl Tonemapper {
    pub fn new(ctx: &mut TextureUploadContext) -> anyhow::Result<Self> {
        let mapface = include_bytes!("../../../../assets/render/tony_mc_mapface.dds");

        let ddsface = ddsfile::Dds::read(io::Cursor::new(&mapface)).unwrap();
        let headerface = ddsface.header;
        let headerface10 = ddsface.header10.unwrap();

        assert_eq!(headerface.width, 48);
        assert_eq!(headerface.height, 48);
        assert_eq!(headerface.depth, Some(48));
        assert_eq!(headerface10.dxgi_format, ddsfile::DxgiFormat::R9G9B9E5_SharedExp);
        assert_eq!(headerface10.resource_dimension, ddsfile::D3D10ResourceDimension::Texture3D);

        let lut_index = Renderer::upload_image_impl(
            ctx,
            Asset::uncached_asset(crate::TextureDescriptor {
                size: UVec2::new(headerface.width, headerface.height),
                mipmaps: 1,
                depth: headerface.depth.unwrap() as u16,
                dimension: crate::TextureDimension::D3,
                format: common::AssetTextureFormat::Rgb9e5,
                path: String::from("tony_mc_mapface ktx2"),
                data: ddsface.data,
            }),
        );

        let ranges = [
            D3D12_DESCRIPTOR_RANGE1 {
                RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
                NumDescriptors: 1,
                BaseShaderRegister: 0,
                RegisterSpace: 0,
                Flags: D3D12_DESCRIPTOR_RANGE_FLAG_DATA_STATIC_WHILE_SET_AT_EXECUTE,
                OffsetInDescriptorsFromTableStart: lut_index.0,
            },
            D3D12_DESCRIPTOR_RANGE1 {
                RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
                NumDescriptors: RESOURCE_HEAP_SIZE,
                BaseShaderRegister: 0,
                RegisterSpace: 1,
                Flags: D3D12_DESCRIPTOR_RANGE_FLAG_DESCRIPTORS_VOLATILE,
                OffsetInDescriptorsFromTableStart: 0,
            },
        ];

        let parameters = [
            D3D12_ROOT_PARAMETER1 {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
                ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
                Anonymous: D3D12_ROOT_PARAMETER1_0 {
                    Constants: D3D12_ROOT_CONSTANTS {
                        ShaderRegister: 0,
                        RegisterSpace: 0,
                        Num32BitValues: 1,
                    },
                },
            },
            D3D12_ROOT_PARAMETER1 {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
                ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
                Anonymous: D3D12_ROOT_PARAMETER1_0 {
                    DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE1 {
                        NumDescriptorRanges: ranges.len() as _,
                        pDescriptorRanges: ranges.as_ptr(),
                    },
                },
            },
        ];

        let sampler = D3D12_STATIC_SAMPLER_DESC {
            Filter: D3D12_FILTER_MIN_MAG_MIP_LINEAR,
            AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
            AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
            AddressW: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
            MipLODBias: 0.0,
            MaxAnisotropy: 1,
            ComparisonFunc: D3D12_COMPARISON_FUNC_NONE,
            BorderColor: D3D12_STATIC_BORDER_COLOR_TRANSPARENT_BLACK,
            MinLOD: 0.0,
            MaxLOD: D3D12_FLOAT32_MAX,
            ShaderRegister: 0,
            RegisterSpace: 0,
            ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
        };

        let root_signature = create_root_signature(ctx.device, &parameters, &[sampler])?;

        let vs_dxil = include_bytes!("../../shaders/dxil/tonemap.vs.cso");
        let ps_dxil = include_bytes!("../../shaders/dxil/tonemap.ps.cso");

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
                CullMode: D3D12_CULL_MODE_NONE,
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
            ctx.device
                .CreateGraphicsPipelineState(&pipeline_desc)
                .context("Failed to create pipeline state")?
        };
        pipeline_state.set_name("Tonemapping");

        Ok(Self { root_signature, pipeline_state })
    }

    /// Assumes that the command list contains a drawable render target.
    pub fn render(
        &self,
        command_list: &ID3D12GraphicsCommandList,
        heap: &DescriptorHeap,
        src_texture: &Texture,
    ) {
        unsafe {
            command_list.SetGraphicsRootSignature(&self.root_signature);
            command_list.SetPipelineState(&self.pipeline_state);
            command_list.SetGraphicsRoot32BitConstant(0, src_texture.srv_descriptor().index, 0);
            command_list.SetGraphicsRootDescriptorTable(1, heap.base_gpu_handle());
            command_list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            command_list.DrawInstanced(3, 1, 0, 0);
        }
    }
}
