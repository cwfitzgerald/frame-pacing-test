#include "common/mcmapface.hlsl"

cbuffer ObjectIndex : register(b0, space0) {
    uint input_texture_index;
}

SamplerState sampler_linear_clamp : register(s0, space0);

Texture3D<float3> tony_mc_mapface_lut : register(t0, space0);
Texture2D<float4> textures[1024] : register(t0, space1);

struct VSOutput {
    float4 position : SV_POSITION;
};

VSOutput vs_main(uint vertexId: SV_VertexID) {
    VSOutput result;
    switch (vertexId) {
        case 0:
            result.position = float4(-1.0, -1.0, 0.0, 1.0);
            break;
        case 1:
            result.position = float4(3.0, -1.0, 0.0, 1.0);
            break;
        case 2:
            result.position = float4(-1.0, 3.0, 0.0, 1.0);
            break;
    }
    return result;
}

float4 ps_main(float4 position: SV_Position) : SV_Target0 {
    Texture2D<float4> input_texture = textures[input_texture_index];

    float4 hdr_value = input_texture.Load(int3(int2(round(position.xy - 0.5)), 0));

    float3 sdr_value = tony_mc_mapface(tony_mc_mapface_lut, sampler_linear_clamp, hdr_value.xyz);

    return float4(sdr_value, 1.0);
}
