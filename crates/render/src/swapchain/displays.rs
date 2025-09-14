use std::ffi::CString;

use windows::{
    core::{Interface, PCSTR},
    Win32::{
        Foundation::TRUE,
        Graphics::{
            Dxgi::{Common::*, *},
            Gdi::{EnumDisplaySettingsA, DEVMODEA, ENUM_CURRENT_SETTINGS},
        },
    },
};

pub fn enumerate_display_info(factory: &IDXGIFactory7) {
    unsafe { DXGIDisableVBlankVirtualization().unwrap() };
    // unsafe { DCompositionBoostCompositorClock(true).unwrap() };
    unsafe {
        let mut adapter_idx = 0;
        while let Ok(adapter) = factory.EnumAdapters1(adapter_idx) {
            let adapter_name = String::from_utf16_lossy(&adapter.GetDesc1().unwrap().Description)
                .trim_end_matches(char::from(0))
                .to_string();
            let mut output_idx = 0;
            while let Ok(output) = adapter.EnumOutputs(output_idx) {
                let output6 = output.cast::<IDXGIOutput6>().unwrap();

                if let Ok(output_desc) = output6.GetDesc1() {
                    let name = String::from_utf16_lossy(&output_desc.DeviceName);
                    let name = name.trim_end_matches(char::from(0));
                    let cname = CString::new(name).unwrap();

                    let mut dev_mode = DEVMODEA::default();
                    assert_eq!(
                        EnumDisplaySettingsA(
                            PCSTR(cname.as_ptr() as *const u8),
                            ENUM_CURRENT_SETTINGS,
                            &mut dev_mode,
                        ),
                        TRUE
                    );

                    let mut num_modes = 0;
                    output6
                        .GetDisplayModeList1(
                            DXGI_FORMAT_R8G8B8A8_UNORM,
                            DXGI_ENUM_MODES_SCALING,
                            &mut num_modes,
                            None,
                        )
                        .unwrap();
                    let mut modes = vec![DXGI_MODE_DESC1::default(); num_modes as usize];
                    output6
                        .GetDisplayModeList1(
                            DXGI_FORMAT_R8G8B8A8_UNORM,
                            DXGI_ENUM_MODES_SCALING,
                            &mut num_modes,
                            Some(modes.as_mut_ptr()),
                        )
                        .unwrap();

                    let mut found_mode = None;
                    for mode in &modes {
                        let freq =
                            mode.RefreshRate.Numerator as f32 / mode.RefreshRate.Denominator as f32;
                        if mode.Width == dev_mode.dmPelsWidth as u32
                            && mode.Height == dev_mode.dmPelsHeight as u32
                            && (freq - dev_mode.dmDisplayFrequency as f32).abs() < 1.0
                        {
                            found_mode = Some(mode);
                            break;
                        }
                    }

                    let found_mode = found_mode.unwrap();

                    println!(
                        "Display {} on {}: {} ({}x{}): {:?} Hz",
                        output_idx,
                        adapter_name,
                        name,
                        output_desc.DesktopCoordinates.right - output_desc.DesktopCoordinates.left,
                        output_desc.DesktopCoordinates.bottom - output_desc.DesktopCoordinates.top,
                        found_mode.RefreshRate.Numerator as f32
                            / found_mode.RefreshRate.Denominator as f32
                    );
                }
                output_idx += 1;
            }
            adapter_idx += 1;
        }
    }
}
