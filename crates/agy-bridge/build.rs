//! Build script for agy-bridge.
//!
//! When the `native` feature is enabled:
//! 1. Compiles the hand-built protobuf definitions (`proto/content.proto`, `proto/localharness.proto`)
//!    using `prost-build`.
//! 2. Ensures the local proxy runtime binary (`localharness`) is available for the target platform,
//!    downloading and extracting it from the official Google Antigravity SDK wheel on `PyPI` if not already present.

#[cfg(feature = "native")]
use std::{
    env,
    fs::File,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

// NOLINT: main returns Result for feature-conditional compilation
#[allow(clippy::unnecessary_wraps)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/content.proto");
    println!("cargo:rerun-if-changed=proto/localharness.proto");
    println!("cargo:rerun-if-env-changed=ANTIGRAVITY_HARNESS_PATH");

    #[cfg(feature = "native")]
    {
        compile_protos()?;
        resolve_or_download_binary()?;
    }

    Ok(())
}

#[cfg(feature = "native")]
fn compile_protos() -> Result<(), Box<dyn std::error::Error>> {
    if env::var("PROTOC").is_err()
        && let Ok(path) = protoc_bin_vendored::protoc_bin_path()
    {
        // SAFETY: build.rs is single-threaded at this point.
        unsafe {
            env::set_var("PROTOC", path);
        }
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let proto_dir = manifest_dir.join("proto");
    let content_proto = proto_dir.join("content.proto");
    let localharness_proto = proto_dir.join("localharness.proto");

    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let descriptor_path = out_dir.join("proto_descriptor.bin");

    let mut prost_config = prost_build::Config::new();
    prost_config
        .file_descriptor_set_path(&descriptor_path)
        .compile_well_known_types()
        .extern_path(".google.protobuf", "::pbjson_types");
    prost_config.compile_protos(&[localharness_proto, content_proto], &[proto_dir])?;

    let descriptor_set = std::fs::read(descriptor_path)?;
    pbjson_build::Builder::new()
        .register_descriptors(&descriptor_set)?
        .build(&[".antigravity.localharness", ".genai"])?;

    Ok(())
}

#[cfg(feature = "native")]
fn resolve_or_download_binary() -> Result<(), Box<dyn std::error::Error>> {
    const ENV_HARNESS_PATH: &str = "ANTIGRAVITY_HARNESS_PATH";
    const ENV_HOME: &str = "HOME";
    const GEMINI_CACHE_DIR: &str = ".gemini";
    const ANTIGRAVITY_DIR: &str = "antigravity";
    const BIN_DIR: &str = "bin";

    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let is_windows = env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows");
    let bin_name = if is_windows {
        "localharness.exe"
    } else {
        "localharness"
    };
    let target_bin_path = out_dir.join(bin_name);

    // 1. Check if already extracted in OUT_DIR
    if target_bin_path.is_file() {
        println!(
            "cargo:rustc-env=NATIVE_HARNESS_PATH={}",
            target_bin_path.display()
        );
        return Ok(());
    }

    // 2. Check ANTIGRAVITY_HARNESS_PATH env var
    // NOLINT: environment variable is optional
    if let Ok(custom_path) = env::var(ENV_HARNESS_PATH) {
        let p = PathBuf::from(custom_path);
        if p.is_file() {
            println!("cargo:rustc-env=NATIVE_HARNESS_PATH={}", p.display());
            return Ok(());
        }
    }

    // 3. Check ~/.gemini/antigravity/bin cache
    if let Some(home) = env::var_os(ENV_HOME).map(PathBuf::from) {
        let candidate = home
            .join(GEMINI_CACHE_DIR)
            .join(ANTIGRAVITY_DIR)
            .join(BIN_DIR)
            .join(bin_name);
        if candidate.is_file() {
            if let Err(e) = std::fs::copy(&candidate, &target_bin_path) {
                eprintln!("Failed to copy binary from {}: {}", candidate.display(), e);
            } else {
                #[cfg(unix)]
                set_executable_permission(&target_bin_path)?;
                println!(
                    "cargo:rustc-env=NATIVE_HARNESS_PATH={}",
                    target_bin_path.display()
                );
                return Ok(());
            }
        }
    }

    // 4. Download from PyPI wheel
    let target_os = env::var("CARGO_CFG_TARGET_OS")?;
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH")?;

    println!(
        "cargo:warning=Downloading local proxy binary from PyPI for {target_os}-{target_arch}..."
    );
    download_and_extract_wheel(&target_os, &target_arch, &target_bin_path)?;

    println!(
        "cargo:rustc-env=NATIVE_HARNESS_PATH={}",
        target_bin_path.display()
    );
    Ok(())
}

#[cfg(feature = "native")]
fn download_and_extract_wheel(
    target_os: &str,
    target_arch: &str,
    dest_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let platform_tag = match (target_os, target_arch) {
        ("linux", "x86_64") => "manylinux_2_17_x86_64",
        ("linux", "aarch64") => "manylinux_2_17_aarch64",
        ("macos", "aarch64") => "macosx_11_0_arm64",
        ("macos", "x86_64") => "macosx_11_0_x86_64",
        ("windows", "x86_64") => "win_amd64",
        ("windows", "aarch64") => "win_arm64",
        (os, arch) => {
            return Err(format!("Unsupported target platform: {os}-{arch}").into());
        }
    };

    // Query PyPI JSON API for google-antigravity
    let pypi_url = "https://pypi.org/pypi/google-antigravity/json";
    let pypi_resp = ureq::get(pypi_url).call()?;
    let json_body: serde_json::Value = pypi_resp.into_body().read_json()?;

    let releases = json_body
        .get("releases")
        .and_then(|r| r.as_object())
        .ok_or("Invalid PyPI response: missing releases")?;

    let version = json_body
        .get("info")
        .and_then(|i| i.get("version"))
        .and_then(|v| v.as_str())
        .ok_or("Invalid PyPI response: missing version")?;

    let files = releases
        .get(version)
        .and_then(|f| f.as_array())
        .ok_or_else(|| format!("No files found for version {version}"))?;

    let wheel_file = files
        .iter()
        .find(|f| {
            let filename = f.get("filename").and_then(|n| n.as_str()).unwrap_or("");
            Path::new(filename)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("whl"))
                && filename.contains(platform_tag)
        })
        .ok_or_else(|| {
            format!("No wheel found matching platform tag '{platform_tag}' for version {version}")
        })?;

    let download_url = wheel_file
        .get("url")
        .and_then(|u| u.as_str())
        .ok_or("Missing download URL for wheel")?;

    let wheel_resp = ureq::get(download_url).call()?;
    let mut wheel_bytes = Vec::new();
    wheel_resp
        .into_body()
        .into_reader()
        .read_to_end(&mut wheel_bytes)?;

    let reader = io::Cursor::new(wheel_bytes);
    let mut zip_archive = zip::ZipArchive::new(reader)?;

    let mut found = false;
    for i in 0..zip_archive.len() {
        let mut file = zip_archive.by_index(i)?;
        let name = file.name().to_string();
        if name.ends_with("google/antigravity/bin/localharness")
            || name.ends_with("google/antigravity/bin/localharness.exe")
        {
            if let Some(parent) = dest_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out = File::create(dest_path)?;
            io::copy(&mut file, &mut out)?;
            out.flush()?;
            found = true;
            break;
        }
    }

    if !found {
        return Err("Could not find localharness binary inside downloaded wheel".into());
    }

    #[cfg(unix)]
    set_executable_permission(dest_path)?;

    Ok(())
}

#[cfg(all(feature = "native", unix))]
fn set_executable_permission(path: &Path) -> Result<(), io::Error> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}
