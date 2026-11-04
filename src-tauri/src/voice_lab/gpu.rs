//! Downloadable CUDA provider, kept separate from the CPU runtime.
use super::{VlError, err, models, trainer};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
};
use tokio::sync::oneshot;

pub const SUPPORTED: bool = cfg!(all(target_os = "linux", target_arch = "x86_64"));
pub const ARCHIVE_URL: &str = "https://github.com/microsoft/onnxruntime/releases/download/v1.28.0/onnxruntime-linux-x64-gpu_cuda13-1.28.0.tgz";
pub const ARCHIVE_SHA256: &str = "84d28f27589090b280d4312743efd3d450cd4ac7d1e1d75e7d9076d9637bf9de";
pub const ARCHIVE_SIZE: u64 = 240_874_643;
pub const ARCHIVE_TOP: &str = "onnxruntime-linux-x64-gpu_cuda13-1.28.0";
pub const FILES: [(&str, &str, &str); 4] = [
    (
        "lib/libonnxruntime_providers_cuda.so",
        "libonnxruntime_providers_cuda.so",
        "051136fa473e255634421b8e1cd61379fc3ed72c34918a60edc5ac15e792f6c0",
    ),
    (
        "lib/libonnxruntime_providers_shared.so",
        "libonnxruntime_providers_shared.so",
        "c6a12593396095f5670160e284c35d1700b7708cf3037b7042e2a5200ccae772",
    ),
    (
        "LICENSE",
        "LICENSE",
        "2f07c72751aed99790b8a4869cf2311df85a860b22ded05fa22803587a48922c",
    ),
    (
        "ThirdPartyNotices.txt",
        "ThirdPartyNotices.txt",
        "0e07b95f3a8d6230037707c5c4a2b554d12c4cb67369669ac255635528ffcee2",
    ),
];
pub fn dir(root: &Path) -> PathBuf {
    root.join("gpu")
}
#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Pack {
    None,
    Ok,
    Broken,
}
fn matches(path: &Path, expected: &str) -> bool {
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let mut hash = Sha256::new();
    let mut buf = [0; 65536];
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => hash.update(&buf[..n]),
            Err(_) => return false,
        }
    }
    format!("{:x}", hash.finalize()) == expected
}
fn pack_with(root: &Path, files: &[(&str, &str, &str)]) -> Pack {
    let gpu = dir(root);
    if files.iter().all(|(_, name, _)| !gpu.join(name).exists()) {
        return Pack::None;
    }
    if files
        .iter()
        .all(|(_, name, sha)| matches(&gpu.join(name), sha))
    {
        Pack::Ok
    } else {
        Pack::Broken
    }
}
/// Hashes the provider files; call on a blocking thread.
pub fn pack(root: &Path) -> Pack {
    pack_with(root, &FILES)
}
pub fn missing_libs(root: &Path) -> Vec<String> {
    let mut missing = Vec::new();
    if let Ok(out) = Command::new("ldd")
        .arg(dir(root).join("libonnxruntime_providers_cuda.so"))
        .output()
        && out.status.success()
    {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if let Some((name, rest)) = line.split_once("=>")
                && rest.trim() == "not found"
            {
                missing.push(name.trim().to_string());
            }
        }
    }
    // Debian users often lack /sbin in PATH.
    if let Some(out) = ["ldconfig", "/sbin/ldconfig", "/usr/sbin/ldconfig"]
        .iter()
        .find_map(|p| Command::new(p).arg("-p").output().ok())
        && out.status.success()
        && !String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .any(|s| s == "libcudnn.so.9")
    {
        missing.push("libcudnn.so.9".into());
    }
    missing.sort();
    missing.dedup();
    missing
}
pub async fn install(
    root: &Path,
    progress: &(dyn Fn(u64, u64) + Send + Sync),
    cancel: oneshot::Receiver<()>,
) -> Result<(), VlError> {
    if !SUPPORTED {
        return Err(err("GPU_UNSUPPORTED"));
    }
    install_from(
        root,
        ARCHIVE_URL,
        ARCHIVE_SHA256,
        ARCHIVE_SIZE,
        ARCHIVE_TOP,
        &FILES,
        progress,
        cancel,
    )
    .await
}
#[allow(clippy::too_many_arguments)]
async fn install_from(
    root: &Path,
    url: &str,
    sha: &str,
    size: u64,
    top: &str,
    files: &[(&str, &str, &str)],
    progress: &(dyn Fn(u64, u64) + Send + Sync),
    mut cancel: oneshot::Receiver<()>,
) -> Result<(), VlError> {
    let tmp = dir(root).join("tmp");
    let result = async {
        if trainer::disk_free_mb(root).is_some_and(|mb| mb < 600) {
            return Err(err("DISK_FULL"));
        }
        let archive = tmp.join("archive.tgz");
        let downloads = [(url.into(), archive.clone(), sha.into(), size)];
        tokio::select! {
            biased;
            _ = &mut cancel => return Err(err("JOB_CANCELLED")),
            r = models::install_files(&downloads, &tmp, progress) => r?,
        }
        let extract = tmp.join("x");
        let files: Vec<_> = files
            .iter()
            .map(|(p, n, s)| (p.to_string(), n.to_string(), s.to_string()))
            .collect();
        let files_for_cancel = files.clone();
        let gpu = dir(root);
        let top = top.to_string();
        // Await the blocking operation before cleanup, so no tar process can recreate tmp afterwards.
        tokio::task::spawn_blocking(move || -> Result<(), VlError> {
            fs::create_dir_all(&extract).map_err(|_| err("GPU_PACK_INVALID"))?;
            let status = Command::new("tar")
                .arg("-xzf")
                .arg(archive)
                .arg("-C")
                .arg(&extract)
                .arg("--strip-components=1")
                .args(files.iter().map(|(p, _, _)| format!("{top}/{p}")))
                .output()
                .map_err(|_| err("GPU_PACK_INVALID"))?;
            if !status.status.success()
                || !files.iter().all(|(p, _, s)| matches(&extract.join(p), s))
            {
                return Err(err("GPU_PACK_INVALID"));
            }
            for (p, n, _) in &files {
                fs::rename(extract.join(p), gpu.join(n)).map_err(|_| err("GPU_PACK_INVALID"))?;
            }
            Ok(())
        })
        .await
        .map_err(|_| err("GPU_PACK_INVALID"))??;
        // A cancel pressed during extraction must not report success.
        if cancel.try_recv().is_ok() {
            for (_, n, _) in files_for_cancel {
                let _ = fs::remove_file(dir(root).join(n));
            }
            return Err(err("JOB_CANCELLED"));
        }
        Ok(())
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&tmp).await;
    result
}
pub fn uninstall(root: &Path) -> Result<(), VlError> {
    match fs::remove_dir_all(dir(root)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::{fs, path::Path, process::Command, time::Duration};
    use tempfile::tempdir;
    use tokio::sync::oneshot;
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::path};

    fn sha(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }
    fn files() -> Vec<(&'static str, &'static str, String)> {
        [
            ("lib/a.so", "a.so"),
            ("lib/b.so", "b.so"),
            ("LICENSE", "LICENSE"),
        ]
        .into_iter()
        .map(|(a, b)| (a, b, sha(b"abc")))
        .collect()
    }
    fn refs<'a>(
        files: &'a [(&'static str, &'static str, String)],
    ) -> Vec<(&'a str, &'a str, &'a str)> {
        files.iter().map(|(a, b, c)| (*a, *b, c.as_str())).collect()
    }
    fn populate(root: &Path) {
        fs::create_dir_all(dir(root)).unwrap();
        for (_, name, _) in files() {
            fs::write(dir(root).join(name), b"abc").unwrap();
        }
    }
    #[test]
    fn status_none_without_files() {
        let d = tempdir().unwrap();
        assert_eq!(pack_with(d.path(), &refs(&files())), Pack::None);
    }
    #[test]
    fn status_ok_with_matching_files() {
        let d = tempdir().unwrap();
        populate(d.path());
        assert_eq!(pack_with(d.path(), &refs(&files())), Pack::Ok);
    }
    #[test]
    fn status_reports_broken_on_hash_mismatch() {
        let d = tempdir().unwrap();
        populate(d.path());
        fs::write(dir(d.path()).join("a.so"), b"bad").unwrap();
        assert_eq!(pack_with(d.path(), &refs(&files())), Pack::Broken);
    }
    #[test]
    fn status_broken_when_one_file_missing() {
        let d = tempdir().unwrap();
        populate(d.path());
        fs::remove_file(dir(d.path()).join("a.so")).unwrap();
        assert_eq!(pack_with(d.path(), &refs(&files())), Pack::Broken);
    }
    #[test]
    fn uninstall_removes_dir() {
        let d = tempdir().unwrap();
        populate(d.path());
        uninstall(d.path()).unwrap();
        assert!(!dir(d.path()).exists());
    }

    async fn archive_server(delay: Duration) -> (MockServer, Vec<u8>) {
        let d = tempdir().unwrap();
        let top = d.path().join("TOP");
        fs::create_dir_all(top.join("lib")).unwrap();
        for (path, _, _) in files() {
            fs::write(top.join(path), b"abc").unwrap();
        }
        fs::write(top.join("extra.bin"), b"extra").unwrap();
        let archive = d.path().join("test.tgz");
        assert!(
            Command::new("tar")
                .arg("-czf")
                .arg(&archive)
                .arg("-C")
                .arg(d.path())
                .arg("TOP")
                .status()
                .unwrap()
                .success()
        );
        let bytes = fs::read(archive).unwrap();
        let server = MockServer::start().await;
        Mock::given(path("/pack"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(bytes.clone())
                    .set_delay(delay),
            )
            .mount(&server)
            .await;
        (server, bytes)
    }
    #[tokio::test]
    async fn install_extracts_only_named_files() {
        let (s, b) = archive_server(Duration::ZERO).await;
        let d = tempdir().unwrap();
        let (_tx, rx) = oneshot::channel();
        install_from(
            d.path(),
            &format!("{}/pack", s.uri()),
            &sha(&b),
            b.len() as u64,
            "TOP",
            &refs(&files()),
            &|_, _| {},
            rx,
        )
        .await
        .unwrap();
        assert_eq!(pack_with(d.path(), &refs(&files())), Pack::Ok);
        assert_eq!(fs::read_dir(dir(d.path())).unwrap().count(), 3);
        assert!(!dir(d.path()).join("tmp").exists());
    }
    #[tokio::test]
    async fn install_rejects_wrong_file_hash() {
        let (s, b) = archive_server(Duration::ZERO).await;
        let d = tempdir().unwrap();
        let (_tx, rx) = oneshot::channel();
        let mut f = files();
        f[1].2 = sha(b"wrong");
        let e = install_from(
            d.path(),
            &format!("{}/pack", s.uri()),
            &sha(&b),
            b.len() as u64,
            "TOP",
            &refs(&f),
            &|_, _| {},
            rx,
        )
        .await
        .unwrap_err();
        assert_eq!(e.code(), "GPU_PACK_INVALID");
        assert!(!dir(d.path()).join("a.so").exists());
        assert!(!dir(d.path()).join("tmp").exists());
    }
    #[tokio::test]
    async fn install_cancel_cleans_tmp() {
        let (s, b) = archive_server(Duration::from_secs(5)).await;
        let d = tempdir().unwrap();
        let (tx, rx) = oneshot::channel();
        let url = format!("{}/pack", s.uri());
        let hash = sha(&b);
        let f = files();
        let r = refs(&f);
        let future = install_from(
            d.path(),
            &url,
            &hash,
            b.len() as u64,
            "TOP",
            &r,
            &|_, _| {},
            rx,
        );
        tokio::pin!(future);
        let cancel = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            tx.send(()).unwrap();
        };
        let (result, ()) = tokio::join!(future, cancel);
        assert_eq!(result.unwrap_err().code(), "JOB_CANCELLED");
        assert!(!dir(d.path()).join("tmp").exists());
    }
}
