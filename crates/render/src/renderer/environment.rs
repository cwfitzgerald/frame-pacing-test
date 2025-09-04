use std::ffi::c_void;

use anyhow::Context;
use windows::Win32::{
    Foundation::*,
    Graphics::{Direct3D::*, Direct3D12::*, Dxgi::Common::*},
};

use crate::{
    renderer::create_root_signature,
    resource::DescriptorHeap,
    util::{ID3D12ObjectExt, InterfaceExt},
    TexturePointer,
};

pub struct EnvironmentMapper {
    root_signature: ID3D12RootSignature,
    pipeline_state: ID3D12PipelineState,
}

impl EnvironmentMapper {
    pub fn new(device: &ID3D12Device) -> anyhow::Result<Self> {
        let ranges = [D3D12_DESCRIPTOR_RANGE1 {
            RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
            NumDescriptors: 1,
            BaseShaderRegister: 0,
            RegisterSpace: 0,
            Flags: D3D12_DESCRIPTOR_RANGE_FLAG_DATA_STATIC_WHILE_SET_AT_EXECUTE,
            OffsetInDescriptorsFromTableStart: 0,
        }];

        let parameters = [
            D3D12_ROOT_PARAMETER1 {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_CBV,
                ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
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

        let vs_dxil = include_bytes!("../../shaders/dxil/environment.vs.cso");
        let ps_dxil = include_bytes!("../../shaders/dxil/environment.ps.cso");

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
                DepthClipEnable: TRUE,
                ..Default::default()
            },
            PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
            NumRenderTargets: 1,
            DSVFormat: DXGI_FORMAT_D32_FLOAT,
            DepthStencilState: D3D12_DEPTH_STENCIL_DESC {
                DepthEnable: TRUE,
                DepthWriteMask: D3D12_DEPTH_WRITE_MASK_ZERO,
                DepthFunc: D3D12_COMPARISON_FUNC_GREATER_EQUAL,
                StencilEnable: FALSE,
                ..Default::default()
            },
            RTVFormats: {
                let mut formats = [DXGI_FORMAT::default(); 8];
                formats[0] = DXGI_FORMAT_R16G16B16A16_FLOAT;
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
        pipeline_state.set_name("Environment");

        Ok(Self { root_signature, pipeline_state })
    }

    /// Assumes that the command list contains a drawable render target.
    pub fn render(
        &self,
        command_list: &ID3D12GraphicsCommandList,
        heap: &DescriptorHeap,
        constant_buffer_address: u64,
        environment_map: TexturePointer,
    ) {
        unsafe {
            command_list.SetGraphicsRootSignature(&self.root_signature);
            command_list.SetPipelineState(&self.pipeline_state);
            command_list.SetGraphicsRootConstantBufferView(0, constant_buffer_address);
            command_list.SetGraphicsRootDescriptorTable(1, heap.gpu_handle(environment_map));
            command_list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            command_list.DrawInstanced(3, 1, 0, 0);
        }
    }
}
