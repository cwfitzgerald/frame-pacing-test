use std::{
    ffi,
    mem::{self, ManuallyDrop},
    ops::Deref,
    panic::catch_unwind,
};

use anyhow::Context;
use windows::{
    core::*,
    Win32::{
        Foundation::{CloseHandle, HANDLE},
        Graphics::{Direct3D::*, Direct3D12::*},
        System::Threading::{CreateEventExW, CREATE_EVENT_MANUAL_RESET, EVENT_ALL_ACCESS},
    },
};

pub fn string_from_utf16(array: &[u16]) -> String {
    let null = array.iter().position(|&c| c == 0).unwrap_or(array.len());
    String::from_utf16_lossy(&array[..null])
}

pub fn str_from_blob(blob: &ID3DBlob) -> &str {
    let bytes = array_from_blob(blob);
    std::str::from_utf8(bytes).unwrap()
}

pub fn array_from_blob(blob: &ID3DBlob) -> &[u8] {
    let ptr = unsafe { blob.GetBufferPointer() };
    let size = unsafe { blob.GetBufferSize() };
    unsafe { std::slice::from_raw_parts(ptr as *const u8, size) }
}

#[macro_export]
macro_rules! include_shader {
    (Pixel $name:literal) => {{
        #[cfg(debug_assertions)]
        let s = include_bytes!(concat!("../shaders/dxil/", $name, "-debug.ps.cso"));
        #[cfg(not(debug_assertions))]
        let s = include_bytes!(concat!("../shaders/dxil/", $name, ".ps.cso"));
        s
    }};
    (Vertex $name:literal) => {{
        #[cfg(debug_assertions)]
        let s = include_bytes!(concat!("../shaders/dxil/", $name, "-debug.vs.cso"));
        #[cfg(not(debug_assertions))]
        let s = include_bytes!(concat!("../shaders/dxil/", $name, ".vs.cso"));
        s
    }};
    (Compute $name:literal) => {{
        #[cfg(debug_assertions)]
        let s = include_bytes!(concat!("../shaders/dxil/", $name, "-debug.cs.cso"));
        #[cfg(not(debug_assertions))]
        let s = include_bytes!(concat!("../shaders/dxil/", $name, ".cs.cso"));
        s
    }};
}

pub unsafe fn check_root_signature_version(device: &ID3D12Device) -> D3D_ROOT_SIGNATURE_VERSION {
    let mut feature_data =
        D3D12_FEATURE_DATA_ROOT_SIGNATURE { HighestVersion: D3D_ROOT_SIGNATURE_VERSION_1_1 };

    device
        .CheckFeatureSupport(
            D3D12_FEATURE_ROOT_SIGNATURE,
            &mut feature_data as *mut _ as *mut ffi::c_void,
            mem::size_of::<D3D12_FEATURE_DATA_ROOT_SIGNATURE>() as u32,
        )
        .unwrap();

    feature_data.HighestVersion
}

pub unsafe fn check_feature_levels(device: &ID3D12Device) -> D3D_FEATURE_LEVEL {
    let levels = [
        D3D_FEATURE_LEVEL_12_2,
        D3D_FEATURE_LEVEL_12_1,
        D3D_FEATURE_LEVEL_12_0,
        D3D_FEATURE_LEVEL_11_1,
        D3D_FEATURE_LEVEL_11_0,
    ];

    let mut feature_data = D3D12_FEATURE_DATA_FEATURE_LEVELS {
        NumFeatureLevels: levels.len() as _,
        pFeatureLevelsRequested: levels.as_ptr(),
        MaxSupportedFeatureLevel: D3D_FEATURE_LEVEL_11_0,
    };

    device
        .CheckFeatureSupport(
            D3D12_FEATURE_FEATURE_LEVELS,
            &mut feature_data as *mut _ as *mut ffi::c_void,
            mem::size_of::<D3D12_FEATURE_DATA_FEATURE_LEVELS>() as u32,
        )
        .unwrap();

    feature_data.MaxSupportedFeatureLevel
}

pub unsafe fn check_shader_models(device: &ID3D12Device) -> D3D_SHADER_MODEL {
    let models = [
        D3D_SHADER_MODEL_6_8,
        D3D_SHADER_MODEL_6_7,
        D3D_SHADER_MODEL_6_6,
        D3D_SHADER_MODEL_6_5,
        D3D_SHADER_MODEL_6_4,
        D3D_SHADER_MODEL_6_3,
        D3D_SHADER_MODEL_6_2,
        D3D_SHADER_MODEL_6_1,
        D3D_SHADER_MODEL_6_0,
        D3D_SHADER_MODEL_5_1,
    ];

    let mut feature_data =
        D3D12_FEATURE_DATA_SHADER_MODEL { HighestShaderModel: D3D_SHADER_MODEL(0) };

    for &model in &models {
        feature_data.HighestShaderModel = model;
        let result = device.CheckFeatureSupport(
            D3D12_FEATURE_SHADER_MODEL,
            &mut feature_data as *mut _ as *mut ffi::c_void,
            mem::size_of::<D3D12_FEATURE_DATA_SHADER_MODEL>() as u32,
        );

        if result.is_ok() {
            break;
        }
    }

    feature_data.HighestShaderModel
}

pub unsafe fn check_feature_support<T: Default>(
    device: &ID3D12Device,
    feature: D3D12_FEATURE,
) -> T {
    unsafe {
        let mut feature_data = T::default();
        device
            .CheckFeatureSupport(
                feature,
                &mut feature_data as *mut T as *mut ffi::c_void,
                mem::size_of::<T>() as u32,
            )
            .unwrap();
        feature_data
    }
}

pub fn format_root_signature_version(version: D3D_ROOT_SIGNATURE_VERSION) -> String {
    match version {
        D3D_ROOT_SIGNATURE_VERSION_1_0 => "1.0".to_string(),
        D3D_ROOT_SIGNATURE_VERSION_1_1 => "1.1".to_string(),
        D3D_ROOT_SIGNATURE_VERSION_1_2 => "1.2".to_string(),
        _ => unreachable!(),
    }
}

pub fn format_shader_model(model: D3D_SHADER_MODEL) -> String {
    format!("{}.{}", model.0 >> 4, model.0 & 0xF)
}

pub fn format_feature_level(level: D3D_FEATURE_LEVEL) -> String {
    format!("{}.{}", level.0 >> 12, (level.0 >> 8) & 0xF)
}

pub trait InterfaceExt: Sized {
    #[allow(unused)]
    fn to_unowned<R>(&self) -> ManuallyDrop<R>
    where
        R: Interface,
        for<'a> &'a R: From<&'a Self>;
    #[allow(unused)]
    fn to_option_unowned<R>(&self) -> ManuallyDrop<Option<R>>
    where
        R: Interface,
        for<'a> &'a R: From<&'a Self>;
}

impl<T: Interface> InterfaceExt for T {
    fn to_unowned<R>(&self) -> ManuallyDrop<R>
    where
        R: Interface,
        for<'a> &'a R: From<&'a Self>,
    {
        unsafe { mem::transmute_copy(self) }
    }

    fn to_option_unowned<R>(&self) -> ManuallyDrop<Option<R>>
    where
        R: Interface,
        for<'a> &'a R: From<&'a Self>,
    {
        unsafe { mem::transmute_copy(self) }
    }
}

pub trait ID3D12ObjectExt {
    fn set_name(&self, name: &str);
}

impl<T> ID3D12ObjectExt for T
where
    for<'a> &'a ID3D12Object: From<&'a T>,
{
    fn set_name(&self, name: &str) {
        unsafe {
            let mut utf16: Vec<u16> = name.encode_utf16().collect();
            utf16.push(0);

            let object: &ID3D12Object = self.into();
            object.SetName(PCWSTR(utf16.as_mut_ptr())).unwrap();
        }
    }
}

pub struct SmartNtHandle(HANDLE);

impl SmartNtHandle {
    pub fn new_manual_reset() -> anyhow::Result<Self> {
        Ok(Self(unsafe {
            CreateEventExW(None, None, CREATE_EVENT_MANUAL_RESET, EVENT_ALL_ACCESS.0)
                .context("Failed to create event")?
        }))
    }

    pub fn from_raw(handle: HANDLE) -> Self {
        Self(handle)
    }
}

impl Deref for SmartNtHandle {
    type Target = HANDLE;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for SmartNtHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

const IGNORED_WARNINGS: [D3D12_MESSAGE_ID; 1] =
    [D3D12_MESSAGE_ID_WRITE_COMBINE_PERFORMANCE_WARNING];

pub unsafe extern "system" fn debug_callback(
    category: D3D12_MESSAGE_CATEGORY,
    severity: D3D12_MESSAGE_SEVERITY,
    id: D3D12_MESSAGE_ID,
    pdescription: PCSTR,
    _pcontext: *mut core::ffi::c_void,
) {
    let e = catch_unwind(|| {
        if IGNORED_WARNINGS.contains(&id) {
            return;
        }

        let backtrace = std::backtrace::Backtrace::force_capture();

        let category = match category {
            D3D12_MESSAGE_CATEGORY_APPLICATION_DEFINED => "Application Defined",
            D3D12_MESSAGE_CATEGORY_MISCELLANEOUS => "Miscellaneous",
            D3D12_MESSAGE_CATEGORY_INITIALIZATION => "Initialization",
            D3D12_MESSAGE_CATEGORY_CLEANUP => "Cleanup",
            D3D12_MESSAGE_CATEGORY_COMPILATION => "Compilation",
            D3D12_MESSAGE_CATEGORY_STATE_CREATION => "State Creation",
            D3D12_MESSAGE_CATEGORY_STATE_SETTING => "State Setting",
            D3D12_MESSAGE_CATEGORY_STATE_GETTING => "State Getting",
            D3D12_MESSAGE_CATEGORY_RESOURCE_MANIPULATION => "Resource Manipulation",
            D3D12_MESSAGE_CATEGORY_EXECUTION => "Execution",
            D3D12_MESSAGE_CATEGORY_SHADER => "Shader",
            _ => "Unknown",
        };

        let severity = match severity {
            D3D12_MESSAGE_SEVERITY_CORRUPTION => "Corruption",
            D3D12_MESSAGE_SEVERITY_ERROR => "Error",
            D3D12_MESSAGE_SEVERITY_WARNING => "Warning",
            D3D12_MESSAGE_SEVERITY_INFO => "Info",
            D3D12_MESSAGE_SEVERITY_MESSAGE => "Message",
            _ => "Unknown",
        };

        let id = id.0;

        let description = pdescription.to_string().unwrap();
        log::error!("D3D12 {category} {severity} ({id}): {description}\n{backtrace}");
    });

    if e.is_err() {
        log::error!("Panicked in debug callback!");
    }
}

pub fn transition_barrier(
    resource: &ID3D12Resource,
    state_before: D3D12_RESOURCE_STATES,
    state_after: D3D12_RESOURCE_STATES,
) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: resource.to_option_unowned(),
                StateBefore: state_before,
                StateAfter: state_after,
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
            }),
        },
    }
}
