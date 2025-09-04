use std::{env, fs};

use camino::{Utf8Path, Utf8PathBuf};

fn main() {
    #[cfg(target_os = "windows")]
    {
        println!("cargo::rustc-link-search=native=build/");

        println!("cargo::rustc-check-cfg=cfg(D3D12_SDK_PATH_UP_COUNT, values(\"2\", \"3\"))");

        embed_manifest::embed_manifest(embed_manifest::new_manifest("com.fantasy_rail")).unwrap();

        // The D3D12 SDK path needs to be relative to the binary, so we look at the OUT_DIR
        // and find how many directories we need to go up to get to target.
        //
        // This allows this to work when --target is set and cargo is outputting to target/<target>/debug
        // as opposed to target/debug.
        //
        // A OUT_DIR path may look like:
        //
        // C:\Users\cwfitzgerald\Programming\fantasy-rail\target\x86_64-pc-windows-msvc\debug\build\fantasy-rail-3134417783071d98\out
        //
        // We count the amount of path components in the path from the end until we hit the target directory.
        // In this case there are 6 components. As the binary ends up in target/<target>/debug we need to remove the 3 components
        // from the end, giving us 3 components.
        //
        // This is hack lel.
        let out_dir = Utf8PathBuf::from(std::env::var("OUT_DIR").unwrap());

        let components_to_target = out_dir
            .components()
            .rev()
            .enumerate()
            .find_map(
                |(i, component)| {
                    if component.as_str() == "target" {
                        Some(i + 1)
                    } else {
                        None
                    }
                },
            )
            .unwrap();

        let needed_components = components_to_target - 3;

        println!("cargo::rustc-cfg=D3D12_SDK_PATH_UP_COUNT=\"{needed_components}\"");

        println!("cargo:rustc-link-arg=/EXPORT:D3D12SDKVersion");
        println!("cargo:rustc-link-arg=/EXPORT:D3D12SDKPath");

        let target_dir = find_target_dir();

        fs::copy("build/SDL2.dll", target_dir.join("SDL2.dll")).unwrap();
    }
}

fn find_target_dir() -> Utf8PathBuf {
    let out_dir = env::var("OUT_DIR").unwrap();
    let mut out_dir = Utf8Path::new(&out_dir);

    loop {
        let name = out_dir.file_name();
        out_dir = out_dir.parent().unwrap();

        match name {
            Some("build") => return out_dir.to_owned(),
            Some(_) => (),
            None => panic!("Could not find target directory"),
        }
    }
}
