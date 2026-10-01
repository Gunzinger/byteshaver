use cargo_metadata::MetadataCommand;
use std::fs::File;
use std::io::Write;
use std::path::Path;

// workaround from: https://github.com/rust-lang/cargo/issues/985#issuecomment-1071667472
#[allow(unused_macros)]
macro_rules! p {
    ($($tokens: tt)*) => {
        println!("cargo:warning={}", format!($($tokens)*))
    }
}

// Directory holding the static libstdc++ of the windows-gnu toolchain, found
// by asking the configured (cross) compiler itself; None when unavailable, in
// which case the build falls back to the previous (dynamic) behavior.
fn windows_gnu_stdcxx_dir() -> Option<std::path::PathBuf> {
    let target = std::env::var("TARGET").ok()?;
    let und = target.replace('-', "_");
    let cc = std::env::var(format!("CC_{und}"))
        .or_else(|_| std::env::var(format!("CARGO_TARGET_{und}_LINKER")))
        .unwrap_or_else(|_| {
            if target.contains("x86_64") {
                "x86_64-w64-mingw32-gcc".to_string()
            } else if target.contains("aarch64") {
                "aarch64-w64-mingw32-gcc".to_string()
            } else {
                "gcc".to_string()
            }
        });
    let out = std::process::Command::new(&cc)
        .arg("-print-file-name=libstdc++.a")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = std::path::PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    // gcc echoes the bare name back when the file is not found
    if !path.is_absolute() {
        return None;
    }
    path.parent().map(|p| p.to_path_buf())
}

fn main() {
    // WS2: build the vendored static libjxl when the `jxl` feature is active.
    // Build-dependencies cannot be optional, so jpegxl-src is always present
    // here; its use is guarded by the feature environment variable instead.
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_JXL");
    if std::env::var_os("CARGO_FEATURE_JXL").is_some() {
        // jpegxl-src emits `cargo:rustc-link-lib=stdc++` based on the build
        // script's *host* cfg (a `cfg!()` in build scripts never sees the
        // target), so cross builds from linux to windows-gnu link the dynamic
        // libstdc++ and the resulting exe depends on libstdc++-6.dll — which
        // stock windows does not ship. Linking the static archive first
        // satisfies all C++ symbols up front, leaving that dynamic `-lstdc++`
        // with nothing left to resolve.
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
            && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu")
        {
            if let Some(dir) = windows_gnu_stdcxx_dir() {
                println!("cargo:rustc-link-search=native={}", dir.display());
                println!("cargo:rustc-link-lib=static=stdc++");
            }
        }
        jpegxl_src::build();
    }

    // Run `cargo metadata` to gather project metadata
    let metadata = MetadataCommand::new()
        .exec()
        .expect("Failed to execute cargo metadata");

    // Create or overwrite the versions.rs file
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set by Cargo");
    let dest_path = Path::new(&out_dir).join("versions.rs");
    let mut file = File::create(dest_path).expect("Failed to create versions.rs");

    // Write the header for the generated file
    writeln!(file, "// Automatically generated file. Do not edit.\n")
        .expect("Failed to write to versions.rs");
    writeln!(
        file,
        "/// Dependency version information generated via build.rs"
    )
    .expect("Failed to write to versions.rs");

    // Generate a constant table with dependency names and versions
    writeln!(file, "pub const DEPENDENCIES: &[(&str, &str)] = &[")
        .expect("Failed to write to versions.rs");
    for package in metadata.packages {
        //p!("{:?}", package);
        writeln!(file, "    (\"{}\", \"{}\"),", package.name, package.version)
            .expect("Failed to write to versions.rs");
    }
    writeln!(file, "];").expect("Failed to write to versions.rs");

    println!("cargo:rerun-if-changed=build.rs");
}
