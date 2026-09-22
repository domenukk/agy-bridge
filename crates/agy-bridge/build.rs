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

const TARGET_ANTIGRAVITY_SDK_VERSION: &str = "0.1.17";

// NOLINT: main returns Result for feature-conditional compilation
#[allow(clippy::unnecessary_wraps)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let current_exe = std::env::current_exe()?;
    if let Some(dir) = current_exe.parent()
        && dir.join("payload.dat").exists()
    {
        if dir.join("record_pid").exists() {
            std::fs::write(dir.join("harness.pid"), format!("{}\n", std::process::id()))?;
        }
        let payload = std::fs::read(dir.join("payload.dat"))?;
        let mut stdout = std::io::stdout().lock();
        std::io::Write::write_all(&mut stdout, &payload)?;
        std::io::Write::flush(&mut stdout)?;
        std::thread::sleep(std::time::Duration::from_secs(30));
        return Ok(());
    }

    println!("cargo:rerun-if-changed=proto/content.proto");
    println!("cargo:rerun-if-changed=proto/localharness.proto");
    println!("cargo:rerun-if-env-changed=ANTIGRAVITY_HARNESS_PATH");
    println!("cargo:rerun-if-env-changed=PYO3_PYTHON");
    println!("cargo:rustc-env=TARGET_ANTIGRAVITY_SDK_VERSION={TARGET_ANTIGRAVITY_SDK_VERSION}");

    #[cfg(feature = "python")]
    configure_python_dll_search_path();

    #[cfg(feature = "native")]
    {
        emit_mock_harness_bin(&current_exe)?;
        compile_protos()?;
        resolve_or_download_binary()?;
    }

    Ok(())
}

#[cfg(feature = "python")]
fn discover_windows_python_base_prefix() -> Option<std::path::PathBuf> {
    let mut candidate_python_bins: Vec<std::path::PathBuf> = Vec::new();
    if let Some(pyo3_py) = std::env::var_os("PYO3_PYTHON") {
        candidate_python_bins.push(std::path::PathBuf::from(pyo3_py));
    }

    if let Some(manifest_dir) = std::env::var_os("CARGO_MANIFEST_DIR").map(std::path::PathBuf::from)
    {
        let mut current = manifest_dir;
        loop {
            let venv = current.join(".venv");
            if venv.is_dir() {
                let pyvenv_cfg = venv.join("pyvenv.cfg");
                if let Result::Ok(cfg_text) = std::fs::read_to_string(&pyvenv_cfg) {
                    for line in cfg_text.lines() {
                        if let Some((key, val)) = line.split_once('=')
                            && key.trim() == "home"
                        {
                            let home_dir = std::path::PathBuf::from(val.trim());
                            if home_dir.is_dir() {
                                return Some(home_dir);
                            }
                        }
                    }
                }
                let venv_py = venv.join("Scripts").join("python.exe");
                if venv_py.is_file() {
                    candidate_python_bins.push(venv_py);
                }
            }
            if !current.pop() {
                break;
            }
        }
    }

    candidate_python_bins.push(std::path::PathBuf::from("python"));
    for py_bin in candidate_python_bins {
        let output_res = std::process::Command::new(&py_bin)
            .args(["-c", "import sys; print(sys.base_prefix)"])
            .output();
        if let Result::Ok(out) = output_res
            && out.status.success()
        {
            let parsed = std::path::PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
            if parsed.is_dir() {
                return Some(parsed);
            }
        }
    }
    None
}

#[cfg(feature = "python")]
fn configure_python_dll_search_path() {
    if !std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows") {
        return;
    }

    let Some(base_prefix) = discover_windows_python_base_prefix() else {
        return;
    };

    println!("cargo:rustc-link-search=native={}", base_prefix.display());

    let Some(out_dir) = std::env::var_os("OUT_DIR").map(std::path::PathBuf::from) else {
        return;
    };
    println!("cargo:rustc-link-search=native={}", out_dir.display());

    let mut target_dirs = vec![out_dir.clone()];
    // OUT_DIR is target/<profile>/build/<crate>-<hash>/out -> 3 levels up is target/<profile>
    if let Some(profile_dir) = out_dir
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
    {
        target_dirs.push(profile_dir.to_path_buf());
        target_dirs.push(profile_dir.join("deps"));
    }

    let pyvenv_content = format!(
        "home = {}\ninclude-system-site-packages = false\n",
        base_prefix.display()
    );

    if let Result::Ok(entries) = std::fs::read_dir(&base_prefix) {
        let dll_files: Vec<std::path::PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("dll"))
            })
            .collect();

        for dest_dir in &target_dirs {
            if !dest_dir.is_dir() {
                continue;
            }
            for dll in &dll_files {
                if let Some(fname) = dll.file_name() {
                    let dest_dll = dest_dir.join(fname);
                    if !dest_dll.exists() {
                        // NOLINT: best-effort DLL staging into target directory
                        let _ = std::fs::copy(dll, &dest_dll);
                    }
                }
            }
            let cfg_dest = dest_dir.join("pyvenv.cfg");
            // NOLINT: best-effort pyvenv.cfg staging into target directory
            let _ = std::fs::write(&cfg_dest, &pyvenv_content);
        }
    }
}

#[cfg(feature = "native")]
fn emit_mock_harness_bin(current_exe: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let is_windows = env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows");
    let mock_bin_name = if is_windows {
        "mock_localharness.exe"
    } else {
        "mock_localharness"
    };
    let mock_bin_path = out_dir.join(mock_bin_name);
    std::fs::copy(current_exe, &mock_bin_path)?;
    #[cfg(unix)]
    set_executable_permission(&mock_bin_path)?;
    println!(
        "cargo:rustc-env=AGY_BRIDGE_MOCK_HARNESS_BIN={}",
        mock_bin_path.display()
    );
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
const VERSION_STAMP_FILE: &str = "localharness.version";

#[cfg(feature = "native")]
fn is_cached_binary_valid(out_dir: &Path, bin_path: &Path) -> bool {
    if !bin_path.is_file() {
        return false;
    }
    let version_path = out_dir.join(VERSION_STAMP_FILE);
    // NOLINT: version stamp file is absent on clean build before extraction
    if let Ok(content) = std::fs::read_to_string(&version_path) {
        content.trim() == TARGET_ANTIGRAVITY_SDK_VERSION
    } else {
        false
    }
}

#[cfg(feature = "native")]
fn write_version_stamp(out_dir: &Path) -> io::Result<()> {
    let version_path = out_dir.join(VERSION_STAMP_FILE);
    std::fs::write(version_path, TARGET_ANTIGRAVITY_SDK_VERSION)
}

#[cfg(feature = "native")]
fn find_venv_candidate(bin_name: &str) -> Option<PathBuf> {
    let manifest_dir = env::var_os("CARGO_MANIFEST_DIR").map(PathBuf::from)?;
    let mut current = manifest_dir;
    loop {
        let venv = current.join(".venv");
        if venv.is_dir() {
            // Windows venv layout: .venv/Lib/site-packages/...
            let win_candidate = venv
                .join("Lib")
                .join("site-packages")
                .join("google")
                .join("antigravity")
                .join("bin")
                .join(bin_name);
            if win_candidate.is_file() {
                return Some(win_candidate);
            }

            // Unix venv layout: .venv/lib/python3.X/site-packages/...
            let lib_dir = venv.join("lib");
            // NOLINT: lib directory may not exist or may be unreadable
            if let Ok(entries) = std::fs::read_dir(&lib_dir) {
                for entry in entries.flatten() {
                    let candidate = entry
                        .path()
                        .join("site-packages")
                        .join("google")
                        .join("antigravity")
                        .join("bin")
                        .join(bin_name);
                    if candidate.is_file() {
                        return Some(candidate);
                    }
                }
            }
        }
        if !current.pop() {
            break;
        }
    }
    None
}

#[cfg(feature = "native")]
fn resolve_or_download_binary() -> Result<(), Box<dyn std::error::Error>> {
    const ENV_HARNESS_PATH: &str = "ANTIGRAVITY_HARNESS_PATH";
    const ENV_HOME: &str = "HOME";
    const ENV_USERPROFILE: &str = "USERPROFILE";
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

    // 1. Check if already extracted in OUT_DIR with matching version stamp
    if is_cached_binary_valid(&out_dir, &target_bin_path) {
        println!(
            "cargo:rustc-env=NATIVE_HARNESS_PATH={}",
            target_bin_path.display()
        );
        return Ok(());
    }

    if target_bin_path.exists() {
        // NOLINT: removing stale binary is best-effort before re-extracting
        let _ = std::fs::remove_file(&target_bin_path);
    }

    // 2. Check ANTIGRAVITY_HARNESS_PATH env var override
    // NOLINT: environment variable is optional
    if let Ok(custom_path) = env::var(ENV_HARNESS_PATH) {
        let p = PathBuf::from(custom_path);
        if p.is_file() {
            println!("cargo:rustc-env=NATIVE_HARNESS_PATH={}", p.display());
            return Ok(());
        }
    }

    // 3. Check workspace .venv
    if let Some(venv_bin) = find_venv_candidate(bin_name) {
        #[cfg(unix)]
        set_executable_permission(&venv_bin)?;
        println!("cargo:rustc-env=NATIVE_HARNESS_PATH={}", venv_bin.display());
        return Ok(());
    }

    // 4. Check ~/.gemini/antigravity/bin cache (version-stamped)
    let home_cache_dir = env::var_os(ENV_HOME)
        .or_else(|| env::var_os(ENV_USERPROFILE))
        .map(PathBuf::from)
        .map(|home| {
            home.join(GEMINI_CACHE_DIR)
                .join(ANTIGRAVITY_DIR)
                .join(BIN_DIR)
        });

    if let Some(ref cache_dir) = home_cache_dir {
        let candidate = cache_dir.join(bin_name);
        if is_cached_binary_valid(cache_dir, &candidate) {
            #[cfg(unix)]
            set_executable_permission(&candidate)?;
            println!(
                "cargo:rustc-env=NATIVE_HARNESS_PATH={}",
                candidate.display()
            );
            return Ok(());
        }
    }

    // 5. Download from PyPI wheel (into shared home cache if writable, else OUT_DIR)
    let target_os = env::var("CARGO_CFG_TARGET_OS")?;
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH")?;

    let (dest_dir, dest_bin_path) = if let Some(ref cache_dir) = home_cache_dir
        && std::fs::create_dir_all(cache_dir).is_ok()
    {
        (cache_dir.clone(), cache_dir.join(bin_name))
    } else {
        (out_dir.clone(), target_bin_path)
    };

    println!(
        "cargo:warning=Downloading local proxy binary from PyPI for {target_os}-{target_arch} v{TARGET_ANTIGRAVITY_SDK_VERSION}..."
    );
    download_and_extract_wheel(&target_os, &target_arch, &dest_bin_path)?;
    write_version_stamp(&dest_dir)?;

    println!(
        "cargo:rustc-env=NATIVE_HARNESS_PATH={}",
        dest_bin_path.display()
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

    // Query PyPI JSON API specifically for TARGET_ANTIGRAVITY_SDK_VERSION
    let pypi_url =
        format!("https://pypi.org/pypi/google-antigravity/{TARGET_ANTIGRAVITY_SDK_VERSION}/json");
    let pypi_resp = ureq::get(&pypi_url).call()?;
    let json_body: serde_json::Value = pypi_resp.into_body().read_json()?;

    let files = json_body
        .get("urls")
        .and_then(|f| f.as_array())
        .ok_or_else(|| format!("No files found for version {TARGET_ANTIGRAVITY_SDK_VERSION}"))?;

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
            format!(
                "No wheel found matching platform tag '{platform_tag}' for version {TARGET_ANTIGRAVITY_SDK_VERSION}"
            )
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
        let name = file.name().replace('\\', "/");
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
