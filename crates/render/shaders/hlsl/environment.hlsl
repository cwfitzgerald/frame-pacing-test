#include "common/global_uniforms.hlsl"

cbuffer ObjectIndex : register(b0, space0) {
    GlobalUniforms uniforms;
}

TextureCube<float3> env_map : register(t0, space0);
SamplerState sampler_linear_clamp : register(s0, space0);

struct VSOutput {
    // This uses depth = 0, so it's "behind" everything else.
    float4 position : SV_POSITION;
    // This uses depth = 1, so we can actually run it
    // through the inverse matrix as if we pass depth = 0,
    // we will get an infinite position.
    float3 clip_position : CLIP_POSITION;
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
    result.clip_position = float3(result.position.xy, 1.0);
    return result;
}

float4 ps_main(VSOutput input) : SV_Target0 {
    // This is a vector in view space representing the direction the pixel is facing.
    float4 view_position_pre =
        mul(uniforms.inverse_view_projection_no_transform_matrix, float4(input.clip_position, 1.0));
    float3 view_position = view_position_pre.xyz / view_position_pre.w;

    return float4(env_map.Sample(sampler_linear_clamp, view_position), 1.0);
}
