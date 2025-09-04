#include "common/global_uniforms.hlsl"

cbuffer ObjectIndex : register(b0, space0) {
    uint object_index;
    uint vertex_offset;
}

cbuffer ConstantBuffer : register(b1, space0) {
    GlobalUniforms uniforms;
}

SamplerState samp : register(s0);

struct ObjectData {
    float4x4 model_matrix;
    float4x4 model_view_matrix;

    uint diffuse_texture_index;
    uint metallic_roughness_texture;
    uint padding0;
    uint padding1;
};
StructuredBuffer<ObjectData> object_data_buffers[2] : register(t0, space0);
static const uint OBJECT_DATA_BUFFER_DESCRIPTOR_BASE_INDEX = 0;
struct MeshData {
    float3 vertices;
    float3 normals;
    float2 uvs;
};
StructuredBuffer<MeshData> mesh_data_buffer : register(t2, space0);

struct LightData {
    float3 view;
    float radius;
    float3 color;
    uint _padding0;
};
StructuredBuffer<LightData> light_data_buffers[2] : register(t3, space0);

Texture2D<float4> images[1024] : register(t0, space1);

float3 mat3_inv_scale_squared(float3x3 transform) {
    float3 dots = float3(
        dot(transform[0].xyz, transform[0].xyz),
        dot(transform[1].xyz, transform[1].xyz),
        dot(transform[2].xyz, transform[2].xyz)
    );
    return 1.0 / dots;
}

struct PSInput {
    float4 position : SV_POSITION;
    float3 view_position : POSITION;
    float3 normal : NORMAL;
    float2 uv : COLOR;
};

PSInput vs_main(uint raw_vertex_id: SV_VertexID) {
    uint vertex_id = raw_vertex_id + vertex_offset;

    StructuredBuffer<ObjectData> object_data_buffer = object_data_buffers[uniforms.frame_number];
    ObjectData object_data = object_data_buffer[object_index];

    float3 model_position = mesh_data_buffer[vertex_id].vertices;
    float3 view_position = mul(object_data.model_view_matrix, float4(model_position, 1.0)).xyz;
    float4 clip_position = mul(uniforms.projection_matrix, float4(view_position, 1.0));

    float3x3 model_view_matrix = (float3x3)object_data.model_view_matrix;

    float3 inv_scale_squared = mat3_inv_scale_squared(model_view_matrix);
    float3 normal = normalize(mul(model_view_matrix, inv_scale_squared * mesh_data_buffer[vertex_id].normals));

    PSInput result;
    result.position = clip_position;
    result.view_position = view_position;
    result.normal = normal;
    result.uv = float2(mesh_data_buffer[vertex_id].uvs);

    return result;
}

static const float PI = 3.14159265358979323846;

float3 compute_f0(float3 base_color, float3 metallic, float3 reflectance) {
    return base_color * metallic + (reflectance * (1.0 - metallic));
}

float pow5(float x) {
    float x2 = x * x;
    return x2 * x2 * x;
}

float brdf_d_ggx(float noh, float a) {
    float a2 = a * a;
    float f = (noh * a2 - noh) * noh + 1.0;
    return a2 / (PI * f * f);
}

float3 brdf_f_schlick_vec3(float u, float3 f0, float f90) {
    return f0 + (f90 - f0) * pow5(1.0 - u);
}

float brdf_fd_lambert() {
    return 1.0 / PI;
}

float brdf_v_smith_ggx_correlated(float nov, float nol, float a) {
    float a2 = a * a;
    float ggxl = nov * sqrt((-nol * a2 + nol) * nol + a2);
    float ggxv = nol * sqrt((-nov * a2 + nov) * nov + a2);
    return 0.5 / (ggxl + ggxv);
}

float3 shade_surface(
    float3 view_position,
    float3 pixel_color,
    float3 pixel_normal,
    float3 pixel_f0,
    float pixel_roughness,
    float occlusion,

    float3 light_intensity,
    float3 light_direction
) {
    float3 n = pixel_normal;
    float3 h = normalize(view_position + light_direction);

    float nov = max(dot(n, view_position), 0.00001);
    float nol = saturate(dot(n, light_direction));
    float noh = saturate(dot(n, h));
    float loh = saturate(dot(light_direction, h));

    float f90 = saturate(dot(pixel_f0, (50.0 * 0.33).xxx));

    float d = brdf_d_ggx(noh, pixel_roughness);
    float3 f = brdf_f_schlick_vec3(loh, pixel_f0, f90);
    float v = brdf_v_smith_ggx_correlated(nov, nol, pixel_roughness);

    // TODO: use
    // https://github.com/bevyengine/bevy/blob/ed042e5f9a715c16b5b5ccbaa765deb02c00a4b6/crates/bevy_pbr/src/render/pbr_lighting.wgsl#L311-L321
    float3 energy_compensation = 1.0;

    float3 specular = d * f * v;
    float3 diffuse = pixel_color * brdf_fd_lambert();

    float3 color = (diffuse + specular) * light_intensity * energy_compensation;

    return color * (nol * occlusion);
}

float4 ps_main(PSInput input) : SV_TARGET {
    StructuredBuffer<ObjectData> object_data_buffer = object_data_buffers[uniforms.frame_number];
    ObjectData object_data = object_data_buffer[object_index];

    Texture2D texture = images[object_data.diffuse_texture_index];

    StructuredBuffer<LightData> light_data_buffer = light_data_buffers[uniforms.frame_number];

    float3 albedo = texture.Sample(samp, input.uv).rgb;

    float3 result_color = (0.0).xxx;
    for (uint i = 0; i < uniforms.light_count; i++) {
        LightData light_data = light_data_buffer[i];

        // Delta to light
        float3 light_direction = light_data.view - input.view_position;
        float light_distance = length(light_direction);

        // Attenuate from light and cusp at radius
        // Derivative is 0 at both d = 0 and d = radius
        // Source: https://lisyarus.github.io/blog/graphics/2022/07/30/point-light-attenuation.html
        float radius = light_data.radius;
        float s = saturate(light_distance / radius);
        float s2 = s * s;
        float inv_s2 = 1.0 - s2;
        float attenuation = inv_s2 * inv_s2 / (1.0 + s2);
        float3 intensity = light_data.color.rgb * attenuation;

        // Normalize light direction
        light_direction /= light_distance;

        float3 f0 = compute_f0(albedo, float3(0.0, 0.0, 0.0), float3(0.04, 0.04, 0.04));

        result_color += shade_surface(
            input.view_position,
            albedo,
            input.normal,
            f0,
            0.5,
            1.0,

            intensity,
            light_direction
        );
    }

    return float4(result_color, 0.0);
}
