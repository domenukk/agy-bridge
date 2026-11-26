//! Build script for agy-bridge.
//!
//! When the `native` feature is enabled:
//! 1. Stages the test mock harness binary (`mock_localharness`).
//! 2. Ensures the local proxy runtime binary (`localharness`) is available for the target platform,
//!    downloading, SHA-256 verifying, and extracting it from the official Google Antigravity SDK
//!    wheel on `PyPI` if not already present.

#[cfg(feature = "native")]
use std::{
    env,
    fs::OpenOptions,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use serde as _;

const TARGET_ANTIGRAVITY_SDK_VERSION: &str = "0.1.24";

#[cfg(feature = "native")]
const VERSION_STAMP_FILE: &str = "localharness.version";
#[cfg(feature = "native")]
const SHA256_STAMP_FILE: &str = "localharness.sha256";

#[cfg(feature = "native")]
const MAX_PYPI_JSON_BYTES: u64 = 4 * 1024 * 1024;
#[cfg(feature = "native")]
const MAX_WHEEL_BYTES: u64 = 256 * 1024 * 1024;
#[cfg(feature = "native")]
const MAX_HARNESS_BIN_BYTES: u64 = 512 * 1024 * 1024;

#[cfg(feature = "native")]
const PINNED_WHEEL_SHA256_MACOS_ARM64: &str =
    "9c359bfa7b752ad3417da1a455a114dca643d02bdea7281e8de8d0d0f44c9cca";
#[cfg(feature = "native")]
const PINNED_WHEEL_SHA256_MACOS_X86_64: &str =
    "ff368c84cc712b822c87dd2ac99b8fac47716c44ffa39bd1caf823e62f00a18f";
#[cfg(feature = "native")]
const PINNED_WHEEL_SHA256_LINUX_AARCH64: &str =
    "55c95c04b00c417667d37a000931638c23479d0f1fa0fbe6e89abb8c60057cda";
#[cfg(feature = "native")]
const PINNED_WHEEL_SHA256_LINUX_X86_64: &str =
    "678c5c56a4efa8d94b90df808a93b339cf3ead498b01898272d5833c8c9ad13f";
#[cfg(feature = "native")]
const PINNED_WHEEL_SHA256_WIN_AMD64: &str =
    "cc2d2af2cbc260d9a9f6c95e7b844185dad29b44760c1e3884d2cb001f55adb8";
#[cfg(feature = "native")]
const PINNED_WHEEL_SHA256_WIN_ARM64: &str =
    "28837e53cf361fca8c63edee1d3cc0ba22c08eae3dec594d7f332e320fdcc9b7";

#[cfg(feature = "native")]
const HARNESS_ZIP_ENTRY_UNIX: &str = "google/antigravity/bin/localharness";
#[cfg(feature = "native")]
const HARNESS_ZIP_ENTRY_WINDOWS: &str = "google/antigravity/bin/localharness.exe";

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

    println!("cargo:rerun-if-env-changed=ANTIGRAVITY_HARNESS_PATH");
    println!("cargo:rerun-if-env-changed=PYO3_PYTHON");
    println!("cargo:rustc-env=TARGET_ANTIGRAVITY_SDK_VERSION={TARGET_ANTIGRAVITY_SDK_VERSION}");

    #[cfg(feature = "python")]
    configure_python_dll_search_path();

    #[cfg(feature = "native")]
    {
        emit_mock_harness_bin(&current_exe)?;
        resolve_or_download_binary()?;
    }

    Ok(())
}

#[cfg(feature = "python")]
fn query_python_base_prefix(py_bin: &std::path::Path) -> Option<std::path::PathBuf> {
    let output_res = std::process::Command::new(py_bin)
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
    None
}

#[cfg(feature = "python")]
fn discover_windows_python_base_prefix() -> Option<std::path::PathBuf> {
    // Respect explicit `PYO3_PYTHON` before inspecting any repository `.venv` so
    // developers/CI using a different Python version get the matching DLLs.
    if let Some(pyo3_py) = std::env::var_os("PYO3_PYTHON")
        && !pyo3_py.is_empty()
        && let Some(prefix) = query_python_base_prefix(&std::path::PathBuf::from(pyo3_py))
    {
        return Some(prefix);
    }

    let mut candidate_python_bins: Vec<std::path::PathBuf> = Vec::new();
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
        if let Some(prefix) = query_python_base_prefix(&py_bin) {
            return Some(prefix);
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
            .filter_map(|entry_res| match entry_res {
                Ok(e) => Some(e.path()),
                Err(err) => {
                    println!("cargo:warning=Failed reading directory entry in base_prefix: {err}");
                    None
                }
            })
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
                    if !dest_dll.exists()
                        && let Err(e) = std::fs::copy(dll, &dest_dll)
                    {
                        println!("cargo:warning=Failed to stage DLL {}: {e}", dll.display());
                    }
                }
            }
            let cfg_dest = dest_dir.join("pyvenv.cfg");
            if let Err(e) = std::fs::write(&cfg_dest, &pyvenv_content) {
                println!(
                    "cargo:warning=Failed to stage pyvenv.cfg at {}: {e}",
                    cfg_dest.display()
                );
            }
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
    remove_stale_temp_path(&mock_bin_path)?;
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
fn bytes_to_lower_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        if let Err(err) = write!(out, "{b:02x}") {
            unreachable!("formatting hex byte into String cannot fail: {err}");
        }
    }
    out
}

#[cfg(feature = "native")]
fn sha256_bytes_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    bytes_to_lower_hex(&digest)
}

#[cfg(feature = "native")]
const IO_CHUNK_BYTES: usize = 64 * 1024;

#[cfg(feature = "native")]
fn sha256_file_hex(path: &Path) -> io::Result<String> {
    use sha2::{Digest, Sha256};
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.file_type().is_file() || meta.len() == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "cached localharness binary must be a non-empty regular file",
        ));
    }
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; IO_CHUNK_BYTES];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(bytes_to_lower_hex(&hasher.finalize()))
}

#[cfg(feature = "native")]
fn is_cached_binary_valid(cache_dir: &Path, bin_path: &Path) -> bool {
    let version_path = cache_dir.join(VERSION_STAMP_FILE);
    let sha256_path = cache_dir.join(SHA256_STAMP_FILE);
    let Ok(version_text) = std::fs::read_to_string(&version_path) else {
        return false;
    };
    if version_text.trim() != TARGET_ANTIGRAVITY_SDK_VERSION {
        return false;
    }
    let Ok(expected_sha_raw) = std::fs::read_to_string(&sha256_path) else {
        return false;
    };
    let expected_sha = expected_sha_raw.trim();
    if expected_sha.len() != 64 || !expected_sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return false;
    }
    match sha256_file_hex(bin_path) {
        Ok(actual_sha) => actual_sha.eq_ignore_ascii_case(expected_sha),
        Err(err) => {
            println!(
                "cargo:warning=Cached localharness verification failed at {}: {err}",
                bin_path.display()
            );
            false
        }
    }
}

#[cfg(feature = "native")]
fn write_cache_stamps(cache_dir: &Path, binary_sha256: &str) -> io::Result<()> {
    std::fs::write(
        cache_dir.join(VERSION_STAMP_FILE),
        TARGET_ANTIGRAVITY_SDK_VERSION,
    )?;
    std::fs::write(cache_dir.join(SHA256_STAMP_FILE), binary_sha256)
}

#[cfg(feature = "native")]
fn remove_stale_temp_path(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.file_type().is_dir() && !meta.file_type().is_symlink() {
                std::fs::remove_dir_all(path)
            } else {
                std::fs::remove_file(path)
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
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

    // 1. Check if already extracted in OUT_DIR with matching version + SHA-256 stamps
    if is_cached_binary_valid(&out_dir, &target_bin_path) {
        println!(
            "cargo:rustc-env=NATIVE_HARNESS_PATH={}",
            target_bin_path.display()
        );
        return Ok(());
    }

    if let Err(e) = remove_stale_temp_path(&target_bin_path) {
        println!(
            "cargo:warning=Failed to remove stale harness binary at {}: {e}",
            target_bin_path.display()
        );
    }

    // 2. Check ANTIGRAVITY_HARNESS_PATH env var override
    if let Some(custom_path) = env::var_os(ENV_HARNESS_PATH) {
        let p = PathBuf::from(custom_path);
        if p.is_file() {
            println!("cargo:rustc-env=NATIVE_HARNESS_PATH={}", p.display());
            return Ok(());
        }
    }

    // 3. Check ~/.gemini/antigravity/bin/v<VERSION> cache
    let home_bin_dir = env::var_os(ENV_HOME)
        .or_else(|| env::var_os(ENV_USERPROFILE))
        .map(PathBuf::from)
        .map(|home| {
            home.join(GEMINI_CACHE_DIR)
                .join(ANTIGRAVITY_DIR)
                .join(BIN_DIR)
        });
    let versioned_cache_dir = home_bin_dir
        .as_ref()
        .map(|bin_dir| bin_dir.join(format!("v{TARGET_ANTIGRAVITY_SDK_VERSION}")));

    if let Some(ref cache_dir) = versioned_cache_dir {
        let candidate = cache_dir.join(bin_name);
        if is_cached_binary_valid(cache_dir, &candidate) {
            #[cfg(unix)]
            set_executable_permission(&candidate)?;
            if let Some(ref shared_bin_dir) = home_bin_dir {
                sync_shared_bin(&candidate, &shared_bin_dir.join(bin_name));
            }
            println!(
                "cargo:rustc-env=NATIVE_HARNESS_PATH={}",
                candidate.display()
            );
            return Ok(());
        }
    }

    // 4. Download from PyPI wheel (into versioned home cache if writable, else OUT_DIR)
    let target_os = env::var("CARGO_CFG_TARGET_OS")?;
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH")?;

    let (dest_dir, dest_bin_path) = if let Some(ref cache_dir) = versioned_cache_dir
        && std::fs::create_dir_all(cache_dir).is_ok()
    {
        (cache_dir.clone(), cache_dir.join(bin_name))
    } else {
        (out_dir.clone(), target_bin_path)
    };

    println!(
        "cargo:warning=Downloading local proxy binary from PyPI for {target_os}-{target_arch} v{TARGET_ANTIGRAVITY_SDK_VERSION}..."
    );
    let binary_sha256 =
        download_and_extract_wheel(&target_os, &target_arch, is_windows, &dest_bin_path)?;
    write_cache_stamps(&dest_dir, &binary_sha256)?;
    if let Some(ref shared_bin_dir) = home_bin_dir {
        sync_shared_bin(&dest_bin_path, &shared_bin_dir.join(bin_name));
    }

    println!(
        "cargo:rustc-env=NATIVE_HARNESS_PATH={}",
        dest_bin_path.display()
    );
    Ok(())
}

#[cfg(feature = "native")]
fn files_have_matching_sha256(a: &Path, b: &Path) -> bool {
    let (Ok(ma), Ok(mb)) = (std::fs::symlink_metadata(a), std::fs::symlink_metadata(b)) else {
        return false;
    };
    if !ma.file_type().is_file() || !mb.file_type().is_file() || ma.len() != mb.len() {
        return false;
    }
    match (sha256_file_hex(a), sha256_file_hex(b)) {
        (Ok(ha), Ok(hb)) => ha.eq_ignore_ascii_case(&hb),
        _ => false,
    }
}

#[cfg(feature = "native")]
fn sync_shared_bin(versioned_bin: &Path, shared_bin: &Path) {
    if files_have_matching_sha256(versioned_bin, shared_bin) {
        return;
    }
    if let Some(parent) = shared_bin.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        println!(
            "cargo:warning=Failed to create shared harness directory {}: {e}",
            parent.display()
        );
        return;
    }
    let tmp = shared_bin.with_extension(format!("tmp.{}", std::process::id()));
    if let Err(e) = remove_stale_temp_path(&tmp) {
        println!(
            "cargo:warning=Failed to remove stale shared temp {}: {e}",
            tmp.display()
        );
        return;
    }
    let copy_result = (|| -> io::Result<()> {
        let mut src = std::fs::File::open(versioned_bin)?;
        let mut dst = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        io::copy(&mut src, &mut dst)?;
        dst.flush()
    })();
    if let Err(e) = copy_result {
        println!(
            "cargo:warning=Failed to stage shared harness temp {}: {e}",
            tmp.display()
        );
        if let Err(rm_err) = remove_stale_temp_path(&tmp) {
            println!(
                "cargo:warning=Failed cleaning up staged harness temp {}: {rm_err}",
                tmp.display()
            );
        }
        return;
    }
    #[cfg(unix)]
    if let Err(e) = set_executable_permission(&tmp) {
        println!(
            "cargo:warning=Failed to set executable permissions on {}: {e}",
            tmp.display()
        );
    }
    if let Err(e) = std::fs::rename(&tmp, shared_bin) {
        println!(
            "cargo:warning=Failed to atomically update shared harness {}: {e}",
            shared_bin.display()
        );
        if let Err(rm_err) = remove_stale_temp_path(&tmp) {
            println!(
                "cargo:warning=Failed cleaning up staged harness temp {}: {rm_err}",
                tmp.display()
            );
        }
    }
}

#[cfg(feature = "native")]
#[derive(serde::Deserialize)]
struct PypiReleaseResponse {
    #[serde(default)]
    urls: Vec<PypiFileEntry>,
}

#[cfg(feature = "native")]
#[derive(Default, serde::Deserialize)]
struct PypiDigests {
    #[serde(default)]
    sha256: String,
}

#[cfg(feature = "native")]
#[derive(serde::Deserialize)]
struct PypiFileEntry {
    #[serde(default)]
    filename: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    digests: PypiDigests,
}

#[cfg(feature = "native")]
fn validate_allowed_pypi_url(url: &str) -> Result<(), String> {
    let Some(after_scheme) = url.strip_prefix("https://") else {
        return Err(format!("Refusing non-HTTPS wheel URL: {url}"));
    };
    let authority = after_scheme.split('/').next().unwrap_or("");
    if authority.is_empty()
        || authority.contains(['@', ':', '?', '#', '\\'])
        || authority.bytes().any(|b| b.is_ascii_whitespace())
    {
        return Err(format!("Invalid authority in PyPI URL: {url}"));
    }
    if !authority.eq_ignore_ascii_case("pypi.org")
        && !authority.eq_ignore_ascii_case("files.pythonhosted.org")
    {
        return Err(format!(
            "Refusing untrusted download host '{authority}' in URL: {url}"
        ));
    }
    Ok(())
}

#[cfg(feature = "native")]
fn read_bounded_bytes<R: Read>(
    reader: R,
    max_bytes: u64,
    label: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut buf = Vec::new();
    let read_len = reader.take(max_bytes + 1).read_to_end(&mut buf)?;
    if read_len as u64 > max_bytes {
        return Err(format!("{label} exceeded maximum allowed size of {max_bytes} bytes").into());
    }
    Ok(buf)
}

#[cfg(feature = "native")]
fn platform_tag_and_pinned_sha256(
    target_os: &str,
    target_arch: &str,
) -> Result<(&'static str, &'static str), String> {
    match (target_os, target_arch) {
        ("linux", "x86_64") => Ok(("manylinux_2_17_x86_64", PINNED_WHEEL_SHA256_LINUX_X86_64)),
        ("linux", "aarch64") => Ok(("manylinux_2_17_aarch64", PINNED_WHEEL_SHA256_LINUX_AARCH64)),
        ("macos", "aarch64") => Ok(("macosx_11_0_arm64", PINNED_WHEEL_SHA256_MACOS_ARM64)),
        ("macos", "x86_64") => Ok(("macosx_11_0_x86_64", PINNED_WHEEL_SHA256_MACOS_X86_64)),
        ("windows", "x86_64") => Ok(("win_amd64", PINNED_WHEEL_SHA256_WIN_AMD64)),
        ("windows", "aarch64") => Ok(("win_arm64", PINNED_WHEEL_SHA256_WIN_ARM64)),
        (os, arch) => Err(format!("Unsupported target platform: {os}-{arch}")),
    }
}

#[cfg(feature = "native")]
fn download_verified_wheel_bytes(
    platform_tag: &str,
    pinned_sha256: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let pypi_url =
        format!("https://pypi.org/pypi/google-antigravity/{TARGET_ANTIGRAVITY_SDK_VERSION}/json");
    validate_allowed_pypi_url(&pypi_url)?;
    let pypi_resp = ureq::get(&pypi_url).call()?;
    let json_bytes = read_bounded_bytes(
        pypi_resp.into_body().into_reader(),
        MAX_PYPI_JSON_BYTES,
        "PyPI release JSON",
    )?;
    let release: PypiReleaseResponse = serde_json::from_slice(&json_bytes)?;

    if release.urls.is_empty() {
        return Err(format!("No files found for version {TARGET_ANTIGRAVITY_SDK_VERSION}").into());
    }

    let wheel_file = release
        .urls
        .iter()
        .find(|f| {
            Path::new(&f.filename)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("whl"))
                && f.filename.contains(platform_tag)
        })
        .ok_or_else(|| {
            format!(
                "No wheel found matching platform tag '{platform_tag}' for version {TARGET_ANTIGRAVITY_SDK_VERSION}"
            )
        })?;

    if wheel_file.url.is_empty() {
        return Err("Missing download URL for wheel".into());
    }
    validate_allowed_pypi_url(&wheel_file.url)?;

    if !wheel_file
        .digests
        .sha256
        .eq_ignore_ascii_case(pinned_sha256)
    {
        return Err(format!(
            "PyPI metadata SHA-256 mismatch for {}: expected pinned {pinned_sha256}, PyPI reported {}",
            wheel_file.filename, wheel_file.digests.sha256
        )
        .into());
    }

    let wheel_resp = ureq::get(&wheel_file.url).call()?;
    let wheel_bytes = read_bounded_bytes(
        wheel_resp.into_body().into_reader(),
        MAX_WHEEL_BYTES,
        &wheel_file.filename,
    )?;

    let actual_sha256 = sha256_bytes_hex(&wheel_bytes);
    if !actual_sha256.eq_ignore_ascii_case(pinned_sha256) {
        return Err(format!(
            "Downloaded wheel SHA-256 mismatch for {}: expected {pinned_sha256}, computed {actual_sha256}",
            wheel_file.filename
        )
        .into());
    }
    Ok(wheel_bytes)
}

#[cfg(feature = "native")]
fn download_and_extract_wheel(
    target_os: &str,
    target_arch: &str,
    is_windows: bool,
    dest_path: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
    let (platform_tag, pinned_sha256) = platform_tag_and_pinned_sha256(target_os, target_arch)?;
    let wheel_bytes = download_verified_wheel_bytes(platform_tag, pinned_sha256)?;
    let expected_entry = if is_windows {
        HARNESS_ZIP_ENTRY_WINDOWS
    } else {
        HARNESS_ZIP_ENTRY_UNIX
    };

    let reader = io::Cursor::new(wheel_bytes);
    let mut zip_archive = zip::ZipArchive::new(reader)?;

    for i in 0..zip_archive.len() {
        let mut file = zip_archive.by_index(i)?;
        if !file.is_file() {
            continue;
        }
        let normalized_entry = file
            .name()?
            .replace('\\', "/")
            .trim_start_matches("./")
            .to_owned();
        if normalized_entry != expected_entry {
            continue;
        }
        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp_path = dest_path.with_extension(format!("tmp.{}", std::process::id()));
        remove_stale_temp_path(&tmp_path)?;
        let binary_sha256 = {
            use sha2::{Digest, Sha256};
            let mut out = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp_path)?;
            let mut limited = (&mut file).take(MAX_HARNESS_BIN_BYTES + 1);
            let mut hasher = Sha256::new();
            let mut total_written = 0u64;
            let mut chunk = vec![0u8; IO_CHUNK_BYTES];
            loop {
                let n = limited.read(&mut chunk)?;
                if n == 0 {
                    break;
                }
                total_written += n as u64;
                if total_written > MAX_HARNESS_BIN_BYTES {
                    drop(out);
                    remove_stale_temp_path(&tmp_path)?;
                    return Err(format!(
                        "Extracted localharness binary exceeded {MAX_HARNESS_BIN_BYTES} bytes"
                    )
                    .into());
                }
                hasher.update(&chunk[..n]);
                out.write_all(&chunk[..n])?;
            }
            out.flush()?;
            bytes_to_lower_hex(&hasher.finalize())
        };
        #[cfg(unix)]
        set_executable_permission(&tmp_path)?;
        remove_stale_temp_path(dest_path)?;
        std::fs::rename(&tmp_path, dest_path)?;
        return Ok(binary_sha256);
    }

    Err(format!("Could not find exact entry '{expected_entry}' inside downloaded wheel").into())
}

#[cfg(all(feature = "native", unix))]
fn set_executable_permission(path: &Path) -> Result<(), io::Error> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to chmod non-regular file",
        ));
    }
    let mut perms = meta.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}
