//! Pinned trainer source and WavLM assets measured during the GPU spike.
use super::models::ModelFile;

pub const TRAINER_REV: &str = "84a6f4c220cd7cc57dce14f638d43e13fa777f42";
pub const WAVLM_REV: &str = "c1423ed94bb01d80a3f5ce5bc39f6026a0f4828c";
/// Version used to generate the frozen Python environment.
pub const MIN_UV: (u32, u32, u32) = (0, 10, 4);

pub const WAVLM_FILES: &[ModelFile] = &[
    ModelFile {
        path: "config.json",
        sha256: "a3d8fe831aaf63d725b54a8ac36f3549cd4365c5086774b2c89cabbc6f9e129d",
        size: 2222,
    },
    ModelFile {
        path: "preprocessor_config.json",
        sha256: "60ca5a31e13f69ee2fbf147504c8676db5f6398fd7a6b12294341dff838edfcf",
        size: 214,
    },
    ModelFile {
        path: "pytorch_model.bin",
        sha256: "fdee460e529396ddb2f8c8e8ce0ad74cfb747b726bc6f612e666c7c1e1963c9d",
        size: 1261990257,
    },
];

use super::{VlError, err, models::install_files};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Mutex,
    time::Duration,
};
use tokio::{process::Command, sync::oneshot};

pub const EMBEDDED: &[(&str, &str)] = &[
    (
        "pyproject.toml",
        include_str!("../../resources/trainer/pyproject.toml"),
    ),
    ("uv.lock", include_str!("../../resources/trainer/uv.lock")),
    (
        "train_job.py",
        include_str!("../../resources/trainer/train_job.py"),
    ),
    (
        "upstream/LICENSE",
        include_str!("../../resources/trainer/upstream/LICENSE"),
    ),
    (
        "upstream/helper.py",
        include_str!("../../resources/trainer/upstream/helper.py"),
    ),
    (
        "upstream/utils_model.py",
        include_str!("../../resources/trainer/upstream/utils_model.py"),
    ),
    (
        "upstream/utils_dataloader.py",
        include_str!("../../resources/trainer/upstream/utils_dataloader.py"),
    ),
    (
        "upstream/utils_style.py",
        include_str!("../../resources/trainer/upstream/utils_style.py"),
    ),
    (
        "upstream/wavlm_loss.py",
        include_str!("../../resources/trainer/upstream/wavlm_loss.py"),
    ),
];

pub fn trainer_dir(root: &Path) -> PathBuf {
    root.join("trainer")
}

pub fn venv_python(root: &Path) -> PathBuf {
    trainer_dir(root).join(if cfg!(windows) {
        ".venv/Scripts/python.exe"
    } else {
        ".venv/bin/python"
    })
}

pub fn write_adapter(root: &Path) -> Result<(), VlError> {
    // Drop files a previous release embedded but this one no longer does.
    match std::fs::remove_dir_all(trainer_dir(root).join("adapter")) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
        _ => {}
    }
    for (name, contents) in EMBEDDED {
        let dest = trainer_dir(root).join("adapter").join(name);
        std::fs::create_dir_all(dest.parent().unwrap())?;
        let tmp = dest.with_extension("tmp");
        std::fs::write(&tmp, contents)?;
        std::fs::rename(tmp, dest)?;
    }
    Ok(())
}

pub fn lock_hash() -> String {
    format!(
        "{:x}",
        Sha256::digest(include_bytes!("../../resources/trainer/uv.lock"))
    )
}

pub fn pack_state(root: &Path) -> &'static str {
    let dir = trainer_dir(root);
    let marker = dir.join("installed.ok");
    if !marker.exists() {
        return "none";
    }
    if std::fs::read_to_string(marker).is_ok_and(|s| s == lock_hash())
        && WAVLM_FILES.iter().all(|file| {
            dir.join("weights/wavlm-large")
                .join(file.path)
                .metadata()
                .is_ok_and(|m| m.is_file() && m.len() == file.size)
        })
    {
        "ok"
    } else {
        "outdated"
    }
}

pub fn find_uv(path_var: Option<&OsStr>, home: Option<&Path>) -> Option<PathBuf> {
    let name = if cfg!(windows) { "uv.exe" } else { "uv" };
    let mut candidates: Vec<_> = path_var
        .into_iter()
        .flat_map(std::env::split_paths)
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.join(name))
        .collect();
    if let Some(home) = home {
        candidates.extend([
            home.join(".local/bin").join(name),
            home.join(".cargo/bin").join(name),
        ]);
    }
    candidates.into_iter().find(|p| {
        p.metadata().is_ok_and(|m| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                m.is_file() && m.permissions().mode() & 0o111 != 0
            }
            #[cfg(not(unix))]
            {
                m.is_file()
            }
        })
    })
}

pub fn parse_uv_version(s: &str) -> Option<(u32, u32, u32)> {
    let mut words = s.split_whitespace();
    if words.next()? != "uv" {
        return None;
    }
    let mut parts = words.next()?.split('.');
    let version = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    if parts.next().is_some() {
        return None;
    }
    Some(version)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuInfo {
    pub name: String,
    pub driver: String,
    pub vram_total_mb: u64,
    pub vram_free_mb: u64,
}

pub fn parse_nvidia_smi(csv: &str) -> Option<GpuInfo> {
    let fields: Vec<_> = csv.lines().next()?.split(',').map(str::trim).collect();
    if fields.len() != 4 || fields[0].is_empty() || fields[1].is_empty() {
        return None;
    }
    Some(GpuInfo {
        name: fields[0].into(),
        driver: fields[1].into(),
        vram_total_mb: fields[2].parse().ok()?,
        vram_free_mb: fields[3].parse().ok()?,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrainStatus {
    pub uv: Option<String>,
    pub uv_ok: bool,
    pub pack: String,
    pub gpu: Option<GpuInfo>,
    pub torch_cuda: Option<String>,
    pub disk_free_mb: Option<u64>,
}

fn command(program: &OsStr) -> Command {
    let mut cmd = Command::new(program);
    cmd.kill_on_drop(true)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    cmd
}

async fn output(cmd: &mut Command, seconds: u64) -> Option<String> {
    let out = tokio::time::timeout(Duration::from_secs(seconds), cmd.output())
        .await
        .ok()?
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8(out.stdout).ok()?.trim().to_string())
}

fn uv_path() -> Option<PathBuf> {
    find_uv(
        std::env::var_os("PATH").as_deref(),
        std::env::var_os("HOME").as_deref().map(Path::new),
    )
}

fn disk_free_mb(root: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let mut existing = root;
        while !existing.exists() {
            existing = existing.parent()?;
        }
        let path = std::ffi::CString::new(existing.as_os_str().as_bytes()).ok()?;
        let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        // statvfs initializes the supplied structure on success.
        if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
            return None;
        }
        let stats = unsafe { stats.assume_init() };
        Some((u128::from(stats.f_bavail) * u128::from(stats.f_frsize) / 1_000_000) as u64)
    }
    #[cfg(not(unix))]
    {
        let _ = root;
        None
    }
}

pub async fn status(
    root: &Path,
    cache: &Mutex<Option<Option<String>>>,
    refresh: bool,
) -> TrainStatus {
    if refresh {
        *cache.lock().unwrap() = None;
    }
    let uv = if let Some(path) = uv_path() {
        output(command(path.as_os_str()).arg("--version"), 10).await
    } else {
        None
    };
    let uv_ok = uv
        .as_deref()
        .and_then(parse_uv_version)
        .is_some_and(|v| v >= MIN_UV);
    let gpu = output(
        command(OsStr::new("nvidia-smi")).args([
            "--query-gpu=name,driver_version,memory.total,memory.free",
            "--format=csv,noheader,nounits",
        ]),
        10,
    )
    .await
    .as_deref()
    .and_then(parse_nvidia_smi);
    let pack = pack_state(root);
    let torch_cuda = if pack == "ok" {
        let cached = cache.lock().unwrap().clone();
        if let Some(value) = cached {
            value
        } else {
            let value = output(
                command(venv_python(root).as_os_str()).args([
                    "-I",
                    "-c",
                    "import torch;print(torch.version.cuda if torch.cuda.is_available() else '')",
                ]),
                60,
            )
            .await
            .filter(|s| !s.is_empty());
            *cache.lock().unwrap() = Some(value.clone());
            value
        }
    } else {
        None
    };
    TrainStatus {
        uv,
        uv_ok,
        pack: pack.into(),
        gpu,
        torch_cuda,
        disk_free_mb: disk_free_mb(root),
    }
}

pub struct UvProgress {
    pub done: u64,
    pub total: u64,
    pub item: Option<String>,
}

pub fn uv_progress(log: &str) -> UvProgress {
    let mut packages = Vec::<(&str, u64)>::new();
    let mut downloaded = std::collections::HashSet::new();
    for line in log
        .split_inclusive('\n')
        .filter(|line| line.ends_with('\n'))
    {
        if let Some(rest) = line.strip_prefix("Downloading ") {
            let Some(name) = rest.split_whitespace().next() else {
                continue;
            };
            let Some((_, size)) = rest.rsplit_once('(') else {
                continue;
            };
            let Some(size) = size.trim().strip_suffix(')') else {
                continue;
            };
            let bytes = [
                ("GiB", 1073741824.0),
                ("MiB", 1048576.0),
                ("KiB", 1024.0),
                ("B", 1.0),
            ]
            .into_iter()
            .find_map(|(unit, scale)| {
                size.strip_suffix(unit)?
                    .parse::<f64>()
                    .ok()
                    .filter(|n| n.is_finite() && *n >= 0.0)
                    .map(|n| (n * scale) as u64)
            });
            if let Some(bytes) = bytes
                && !packages.iter().any(|(n, _)| *n == name)
            {
                packages.push((name, bytes));
            }
        } else if let Some(rest) = line.strip_prefix(" Downloaded ")
            && let Some(name) = rest.split_whitespace().next()
        {
            downloaded.insert(name);
        }
    }
    UvProgress {
        total: packages.iter().map(|(_, bytes)| bytes).sum(),
        done: packages
            .iter()
            .filter(|(name, _)| downloaded.contains(name))
            .map(|(_, bytes)| bytes)
            .sum(),
        item: packages
            .iter()
            .rev()
            .find(|(name, _)| !downloaded.contains(name))
            .map(|(name, _)| (*name).to_string()),
    }
}

#[allow(clippy::type_complexity)] // Keep the install callback signature explicit.
pub async fn install(
    root: &Path,
    progress: &(dyn Fn(&str, u64, u64, Option<&str>) + Send + Sync),
    mut cancel: oneshot::Receiver<()>,
) -> Result<(), VlError> {
    crate::dlog!("train", "install start");
    let result: Result<(), VlError> = async {
        let uv = uv_path().ok_or_else(|| err("UV_MISSING"))?;
        let version = output(command(uv.as_os_str()).arg("--version"), 10)
            .await
            .and_then(|s| parse_uv_version(&s))
            .ok_or_else(|| err("TRAINER_INSTALL_FAILED"))?;
        if version < MIN_UV {
            return Err(err("UV_TOO_OLD"));
        }
        let dir = trainer_dir(root);
        let failed = |_| err("TRAINER_INSTALL_FAILED");
        match std::fs::remove_file(dir.join("installed.ok")) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(err("TRAINER_INSTALL_FAILED")),
        }
        write_adapter(root).map_err(failed)?;
        if disk_free_mb(root).is_some_and(|mb| mb < 9000) {
            return Err(err("TRAINER_INSTALL_FAILED"));
        }
        crate::dlog!("train", "install stage=venv");
        progress("venv", 0, 0, None);
        let mut cmd = command(uv.as_os_str());
        cmd.args(["sync", "--frozen", "--no-config", "--project"])
            .arg(dir.join("adapter"))
            .env("NO_COLOR", "1")
            .args(["--color", "never"])
            .env("UV_PYTHON_DOWNLOADS", "automatic")
            .env("UV_PROJECT_ENVIRONMENT", dir.join(".venv"))
            .env("UV_PYTHON_INSTALL_DIR", dir.join("python"))
            .env("UV_PYTHON_PREFERENCE", "only-managed")
            .env("UV_CACHE_DIR", dir.join("uv-cache"))
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(dir.join("install.log"))?);
        let mut child = cmd.spawn().map_err(|_| err("TRAINER_INSTALL_FAILED"))?;
        let mut interval = tokio::time::interval(Duration::from_millis(500));
        loop {
            tokio::select! {
                result = child.wait() => {
                    let status = result.map_err(|_| err("TRAINER_INSTALL_FAILED"))?;
                    crate::dlog!("train", "uv exit code={:?}", status.code());
                    if !status.success() {
                        return Err(err("TRAINER_INSTALL_FAILED"));
                    }
                    break;
                }
                _ = &mut cancel => {
                    let _ = child.kill().await;
                    return Err(err("JOB_CANCELLED"));
                }
                _ = interval.tick() => {
                    let log = tokio::fs::read(dir.join("install.log")).await.unwrap_or_default();
                    let p = uv_progress(&String::from_utf8_lossy(&log));
                    if p.done < p.total {
                        progress("packages", p.done, p.total, p.item.as_deref());
                    } else {
                        progress("venv", 0, 0, None);
                    }
                }
            }
        }
        progress("venv", 1, 1, None);
        let files: Vec<_> = WAVLM_FILES
            .iter()
            .map(|file| {
                (
                    format!(
                        "https://huggingface.co/microsoft/wavlm-large/resolve/{WAVLM_REV}/{}",
                        file.path
                    ),
                    dir.join("weights/wavlm-large").join(file.path),
                    file.sha256,
                    file.size,
                )
            })
            .collect();
        let tmp = dir.join("tmp");
        crate::dlog!("train", "install stage=weights");
        let weights_progress = |d, t| progress("weights", d, t, None);
        let result = tokio::select! {
            result = install_files(&files, &tmp, &weights_progress) =>
                result.map_err(|_| err("TRAINER_INSTALL_FAILED")),
            _ = &mut cancel => Err(err("JOB_CANCELLED")),
        };
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        result?;
        std::fs::write(dir.join("installed.ok"), lock_hash())
            .map_err(|_| err("TRAINER_INSTALL_FAILED"))?;
        Ok(())
    }
    .await;
    crate::dlog!(
        "train",
        "install status={} code={}",
        match &result {
            Ok(()) => "completed",
            Err(e) if e.code() == "JOB_CANCELLED" => "cancelled",
            Err(_) => "failed",
        },
        result.as_ref().err().map(|e| e.code()).unwrap_or_default()
    );
    result
}

pub fn uninstall(root: &Path) -> Result<(), VlError> {
    match std::fs::remove_dir_all(trainer_dir(root)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    const FIXTURE: &str = include_str!("fixtures/uv_install_partial.log");
    #[test]
    fn uv_progress_on_real_log() {
        let p = uv_progress(FIXTURE);
        assert_eq!(p.total, 3_968_860_149);
        assert_eq!(p.done, 92_589_255);
        assert!(p.done > 0 && p.done < p.total);
        assert_eq!(p.item.as_deref(), Some("triton"));
    }
    #[test]
    fn uv_progress_parses_python_line() {
        let p = uv_progress(
            "Downloading cpython-3.12.12-linux-x86_64-gnu (download) (32.3MiB)\n Downloaded cpython-3.12.12-linux-x86_64-gnu (download)\n",
        );
        assert_eq!(p.done, (32.3 * 1048576.0) as u64);
        assert_eq!(p.done, p.total);
        assert_eq!(p.item, None);
    }
    #[test]
    fn uv_progress_ignores_partial_line() {
        let p = uv_progress("Downloading torch (991.1MiB)\n Downloaded tor");
        assert_eq!(p.done, 0);
        assert_eq!(p.total, (991.1 * 1048576.0) as u64);
        assert_eq!(uv_progress("Downloading torch (991.1MiB)").total, 0);
    }
    #[test]
    fn uv_progress_unique_names() {
        let p = uv_progress(
            "Downloading nvidia-cublas-cu12 (1MiB)\nDownloading nvidia-cublas-cu12 (1MiB)\nDownloading nvidia-cublas (2MiB)\n Downloaded nvidia-cublas\n Downloaded nvidia-cublas\n",
        );
        assert_eq!(p.total, 3 * 1048576);
        assert_eq!(p.done, 2 * 1048576);
        assert_eq!(p.item.as_deref(), Some("nvidia-cublas-cu12"));
    }
    #[test]
    fn uv_progress_ignores_unknown_downloaded() {
        let p = uv_progress(" Downloaded unknown\nDownloading known (512B)\n");
        assert_eq!(p.done, 0);
        assert_eq!(p.total, 512);
    }
    #[test]
    fn uv_progress_units() {
        let p = uv_progress(
            "Downloading a (512B)\nDownloading b (1.5KiB)\nDownloading c (2GiB)\n Downloaded a\n Downloaded b\n Downloaded c\n",
        );
        assert_eq!(p.total, 512 + 1536 + 2 * 1024 * 1024 * 1024);
        assert_eq!(p.done, p.total);
    }
    #[test]
    fn parse_nvidia_smi_line() {
        let gpu = parse_nvidia_smi("NVIDIA GeForce RTX 5070 Ti, 580.95, 16303, 15800").unwrap();
        assert_eq!(gpu.name, "NVIDIA GeForce RTX 5070 Ti");
        assert_eq!(gpu.driver, "580.95");
        assert_eq!((gpu.vram_total_mb, gpu.vram_free_mb), (16303, 15800));
    }

    #[test]
    fn parse_nvidia_smi_empty() {
        for csv in ["", "N/A", "GPU, driver, N/A, 100", "GPU, driver, 100"] {
            assert!(parse_nvidia_smi(csv).is_none());
        }
    }

    #[test]
    fn parse_uv_version_line() {
        assert_eq!(parse_uv_version("uv 0.10.4 (abc 2026-01-01)"), Some(MIN_UV));
        for value in ["", "python 0.10.4", "uv 0.10", "uv a.b.c"] {
            assert_eq!(parse_uv_version(value), None);
        }
    }

    #[test]
    fn find_uv_fallbacks() {
        let dir = tempdir().unwrap();
        let local = dir.path().join(".local/bin/uv");
        let cargo = dir.path().join(".cargo/bin/uv");
        for path in [&local, &cargo] {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        assert_eq!(
            find_uv(Some(std::ffi::OsStr::new("")), Some(dir.path())),
            Some(local.clone())
        );
        fs::remove_file(local).unwrap();
        assert_eq!(find_uv(None, Some(dir.path())), Some(cargo.clone()));
        let path = std::env::join_paths([cargo.parent().unwrap()]).unwrap();
        assert_eq!(find_uv(Some(&path), None), Some(cargo));
        assert_eq!(find_uv(None, None), None);
    }

    #[test]
    fn outdated_when_lock_hash_differs() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(trainer_dir(dir.path())).unwrap();
        let marker = trainer_dir(dir.path()).join("installed.ok");
        fs::write(&marker, "old").unwrap();
        assert_eq!(pack_state(dir.path()), "outdated");
        fs::write(&marker, lock_hash()).unwrap();
        assert_eq!(pack_state(dir.path()), "outdated");
        for file in WAVLM_FILES {
            let dest = trainer_dir(dir.path())
                .join("weights/wavlm-large")
                .join(file.path);
            fs::create_dir_all(dest.parent().unwrap()).unwrap();
            fs::File::create(dest).unwrap().set_len(file.size).unwrap();
        }
        assert_eq!(pack_state(dir.path()), "ok");
    }

    #[test]
    fn pack_none_without_marker() {
        assert_eq!(pack_state(tempdir().unwrap().path()), "none");
    }

    #[test]
    fn write_adapter_writes_embedded() {
        let dir = tempdir().unwrap();
        write_adapter(dir.path()).unwrap();
        write_adapter(dir.path()).unwrap();
        assert_eq!(EMBEDDED.len(), 9);
        for (name, contents) in EMBEDDED {
            assert_eq!(
                fs::read_to_string(trainer_dir(dir.path()).join("adapter").join(name)).unwrap(),
                *contents
            );
        }
        assert_eq!(lock_hash().len(), 64);
        assert!(
            !trainer_dir(dir.path())
                .join("adapter/test_train_job.py")
                .exists()
        );
    }

    #[tokio::test]
    #[ignore = "downloads ~7 GB trainer environment and WavLM; requires uv and NVIDIA CUDA"]
    async fn install_real_pack() {
        let scratch =
            std::env::var_os("TRAINER_TEST_TMP").expect("set TRAINER_TEST_TMP to scratchpad");
        let dir = tempfile::Builder::new()
            .prefix("task4-install-")
            .tempdir_in(scratch)
            .unwrap();
        let (_sender, cancel) = tokio::sync::oneshot::channel();
        let start = std::time::Instant::now();
        install(
            dir.path(),
            &|stage, done, total, _| {
                if done == 0 || done == total {
                    eprintln!("{stage}: {done}/{total}");
                }
            },
            cancel,
        )
        .await
        .unwrap();
        assert_eq!(pack_state(dir.path()), "ok");
        let diagnostics = status(dir.path(), &std::sync::Mutex::new(None), true).await;
        assert_eq!(diagnostics.torch_cuda.as_deref(), Some("12.8"));
        eprintln!(
            "install_real_pack: {:.2}s (including diagnostics)",
            start.elapsed().as_secs_f64()
        );
        uninstall(dir.path()).unwrap();
        assert_eq!(pack_state(dir.path()), "none");
    }
}
