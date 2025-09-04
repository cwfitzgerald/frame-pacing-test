pub fn load_texture(buffer_url: &str) -> crate::TextureDescriptor {
    let image_data = common::assets::load_file(buffer_url).unwrap();

    let ktx = ktx2::Reader::new(&*image_data).unwrap();

    let header = ktx.header();

    let total_data_size = ktx.levels().map(|level| level.len()).sum::<usize>();
    let mut level_data = Vec::with_capacity(total_data_size);
    for level in ktx.levels() {
        level_data.extend_from_slice(level);
    }

    let format = match header.format {
        Some(ktx2::Format::R8G8B8A8_UNORM) => common::AssetTextureFormat::Rgba8,
        Some(ktx2::Format::BC1_RGB_UNORM_BLOCK) => common::AssetTextureFormat::BC1,
        Some(ktx2::Format::BC5_UNORM_BLOCK) => common::AssetTextureFormat::BC5,
        Some(ktx2::Format::BC6H_UFLOAT_BLOCK) => common::AssetTextureFormat::BC6,
        Some(ktx2::Format::BC7_UNORM_BLOCK) => common::AssetTextureFormat::BC7,
        _ => panic!("Unsupported texture format {:?}", header.format),
    };

    // See https://github.khronos.org/KTX-Specification/ktxspec.v2.html#_texture_type
    let dimension = match (header.pixel_depth, header.face_count) {
        (0, 1) => crate::TextureDimension::D2,
        (0, 6) => crate::TextureDimension::Cube,
        (_, 1) => crate::TextureDimension::D3,
        _ => panic!(
            "Unsupported texture dimension: depth={}, faces={}",
            header.pixel_depth, header.face_count
        ),
    };

    let depth = match dimension {
        crate::TextureDimension::D2 => 1,
        crate::TextureDimension::Cube => 6,
        crate::TextureDimension::D3 => header.pixel_depth as u16,
    };

    crate::TextureDescriptor {
        size: glam::UVec2::new(header.pixel_width, header.pixel_height),
        mipmaps: header.level_count as _,
        depth,
        dimension,
        format,
        path: buffer_url.to_string(),
        data: level_data,
    }
}
