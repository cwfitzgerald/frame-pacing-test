float4 unpack_u32(uint32_t packed) {
    return float4((packed & 0xFF), ((packed >> 8) & 0xFF), ((packed >> 16) & 0xFF), ((packed >> 24) & 0xFF)) / 255.0;
}

float4 clip_from_egui(uint2 screen_size, float2 position) {
    return float4(2.0 * position.x / screen_size.x - 1.0, 1.0 - 2.0 * position.y / screen_size.y, 0.0, 1.0);
}

// -----------------------------------------------
// Adapted from
// https://www.shadertoy.com/view/llVGzG
// Originally presented in:
// Jimenez 2014, "Next Generation Post-Processing in Call of Duty"
//
// A good overview can be found in
// https://blog.demofox.org/2022/01/01/interleaved-gradient-noise-a-different-kind-of-low-discrepancy-sequence/
// via https://github.com/rerun-io/rerun/
float interleaved_gradient_noise(float2 n) {
    float f = dot(float2(0.06711056, 0.00583715), n);
    return frac(52.9829189 * frac(f));
}

float3 dither_interleaved(float3 rgb, float levels, float2 frag_coord) {
    float noise = interleaved_gradient_noise(frag_coord);
    // scale down the noise slightly to ensure flat colors aren't getting dithered
    noise = (noise - 0.5) * 0.95;
    return rgb + noise / (levels - 1.0);
}

// Convert sRGB to linear sRGB.
float4 srgb_linear_from_srgb(float4 srgb) {
    float3 srgb3 = srgb.rgb;
    bool3 cutoff3 = srgb3 < 0.04045;
    float3 lower3 = srgb3 / 12.92;
    float3 higher3 = pow((srgb3 + 0.055) / 1.055, 2.4);
    float3 linear3 = lerp(higher3, lower3, float3(cutoff3));
    return float4(linear3, srgb.a);
}

// Convert linear sRGB to sRGB.
float4 srgb_from_srgb_linear(float4 lin) {
    float3 lin3 = lin.rgb;
    bool3 cutoff3 = lin3 < 0.0031308;
    float3 lower3 = lin3 / 12.92;
    float3 higher3 = 1.055 * pow(lin3, 1.0 / 2.4) - 0.055;
    float3 srgb3 = lerp(higher3, lower3, float3(cutoff3));
    return float4(srgb3, lin.a);
}

ByteAddressBuffer vertex_buffer : register(t0, space0);
Texture2D<float4> textures[1024] : register(t0, space1);
SamplerState sampler_linear_clamp : register(s0, space0);

struct Vertex {
    float2 position;
    float2 tex_coord;
    uint32_t color;
};

struct VsOutput {
    float4 position : SV_POSITION;
    float2 tex_coord : TEXCOORD0;
    float4 color : COLOR0;
};

struct Registers {
    uint32_t2 screen_size;
    uint vertex_offset;
    uint texture_id;
};

ConstantBuffer<Registers> registers : register(b0, space0);

VsOutput vs_main(uint32_t vertex_id: SV_VertexID) {
    VsOutput output;
    Vertex vertex = vertex_buffer.Load<Vertex>(registers.vertex_offset + vertex_id * 20);
    output.position = clip_from_egui(registers.screen_size, vertex.position);
    output.tex_coord = vertex.tex_coord;
    output.color = unpack_u32(vertex.color);
    return output;
}

float4 ps_main(VsOutput input) : SV_TARGET0 {
    float4 tex_linear = textures[registers.texture_id].Sample(sampler_linear_clamp, input.tex_coord);
    float4 tex_srgb = srgb_from_srgb_linear(tex_linear);
    float4 out_srgb = input.color * tex_srgb;
    out_srgb.rgb = dither_interleaved(out_srgb.rgb, 256, input.position.xy);
    float4 out_linear = srgb_linear_from_srgb(out_srgb);
    return out_linear;
}
