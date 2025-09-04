#pragma once

struct GlobalUniforms {
    float4x4 projection_matrix;
    float4x4 inverse_view_projection_no_transform_matrix;
    uint frame_number;
    uint light_count;
};
