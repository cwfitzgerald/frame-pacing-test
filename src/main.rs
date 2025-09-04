use std::{
    ffi::{CStr, CString},
    ptr,
};

use anyhow::Context;
use glam::UVec2;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use sdl2::{
    clipboard::ClipboardUtil,
    event::{Event, WindowEvent},
    keyboard::Keycode,
    sys::{SDL_MessageBoxFlags, SDL_ShowSimpleMessageBox},
    Sdl, VideoSubsystem,
};
use windows::Win32::Foundation::HWND;

#[no_mangle]
#[allow(non_upper_case_globals)]
pub static D3D12SDKVersion: u32 = 614;

#[cfg(all(target_arch = "x86_64", D3D12_SDK_PATH_UP_COUNT = "2"))]
#[no_mangle]
#[allow(non_upper_case_globals)]
pub static D3D12SDKPath: &[u8] = b"../../d3d12/x64\0";
#[cfg(all(target_arch = "x86_64", D3D12_SDK_PATH_UP_COUNT = "3"))]
#[no_mangle]
#[allow(non_upper_case_globals)]
pub static D3D12SDKPath: &[u8] = b"../../../d3d12/x64\0";
#[cfg(all(target_arch = "aarch64", D3D12_SDK_PATH_UP_COUNT = "2"))]
#[no_mangle]
#[allow(non_upper_case_globals)]
pub static D3D12SDKPath: &[u8] = b"../../d3d12/aarch64\0";
#[cfg(all(target_arch = "aarch64", D3D12_SDK_PATH_UP_COUNT = "3"))]
#[no_mangle]
#[allow(non_upper_case_globals)]
pub static D3D12SDKPath: &[u8] = b"../../../d3d12/aarch64\0";

fn display_error(clipboard: &ClipboardUtil, error: anyhow::Error) -> ! {
    let error = format!("{error:?}");
    let text_to_display = format!(
        "A fatal error has occurred! The error has been copied to your clipboard.\n\n{}",
        error
    );

    eprintln!("\n{text_to_display}");

    let _ = clipboard.set_clipboard_text(&error);

    let cstr_to_display = CString::new(text_to_display).unwrap();
    let title = CStr::from_bytes_with_nul(b"Fatal Error\0").unwrap();
    unsafe {
        SDL_ShowSimpleMessageBox(
            SDL_MessageBoxFlags::SDL_MESSAGEBOX_ERROR as u32,
            title.as_ptr(),
            cstr_to_display.as_ptr(),
            ptr::null_mut(),
        )
    };

    std::process::exit(1);
}

fn inner_main(sdl_context: &Sdl, video_subsystem: &VideoSubsystem) -> anyhow::Result<()> {
    let mouse = sdl_context.mouse();
    let window = {
        let _span = tracy_client::span!("Create Window");

        video_subsystem
            .window("Fantasy Rail", 1280, 720)
            .resizable()
            .fullscreen_desktop()
            .build()
            .unwrap()
    };

    let mut relative_mode = true;
    mouse.set_relative_mouse_mode(relative_mode);

    let handle = window.window_handle().context("Failed to get window handle")?;

    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        anyhow::bail!("Unsupported platform!");
    };

    let mut args = pico_args::Arguments::from_env();

    let use_adapter: u32 = args.opt_value_from_str("--adapter")?.unwrap_or(0);

    let use_dcomp = args.contains("--dcomp");

    let mut renderer = render::Renderer::new(
        HWND(handle.hwnd.get() as _),
        UVec2::from(window.size()),
        use_dcomp,
        use_adapter,
    )?;

    let mut game = common::Game::new(String::from("scene"));

    game.world.spawn((components::Camera {
        location: glam::Vec3::new(0.0, 0.0, 0.0),
        pan: 0.0,
        tilt: 0.0,
    },));

    let mut event_pump = sdl_context.event_pump().unwrap();

    'quit: loop {
        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. } | Event::KeyDown { keycode: Some(Keycode::Escape), .. } => {
                    break 'quit
                }
                Event::Window { win_event: WindowEvent::Resized(x, y), .. } => {
                    println!("Resized to {}x{}", x, y);
                    renderer.resize(UVec2::new(x as _, y as _))?;
                }
                Event::MouseMotion { xrel, yrel, .. } => {
                    game.input.mouse_delta += glam::Vec2::new(xrel as f32, yrel as f32);
                }
                Event::KeyDown { keycode: Some(sdl2::keyboard::Keycode::LAlt), .. } => {
                    relative_mode = !relative_mode;
                    mouse.set_relative_mouse_mode(relative_mode);
                }
                Event::KeyDown { keycode: Some(keycode), .. } => {
                    game.input.buttons.insert(keycode);
                }
                Event::KeyUp { keycode: Some(keycode), .. } => {
                    game.input.buttons.remove(&keycode);
                }
                _ => {}
            }
        }

        systems::load_scene(&mut game);
        systems::apply_mouse_input(&mut game);
        systems::move_camera(&mut game);

        systems::debug_ui::compute_debug_ui(&mut game);

        renderer.system_load_resources(&mut game);
        renderer.system_render(&mut game);

        tracy_client::frame_mark();
    }

    renderer.destroy();

    Ok(())
}

fn main() {
    tracy_client::Client::start();
    env_logger::Builder::new().filter(None, log::LevelFilter::Info).parse_default_env().init();

    common::initialize();

    let span = tracy_client::span!("SDL2 Init");
    let sdl_context = sdl2::init().unwrap();
    let video_subsystem = sdl_context.video().unwrap();
    drop(span);

    let result = inner_main(&sdl_context, &video_subsystem);

    if let Err(error) = result {
        display_error(&video_subsystem.clipboard(), error)
    }
}
