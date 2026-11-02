//! Pinned upstream assets. SHA-256 and byte sizes measured from downloaded files.
pub const MODEL_REVISION: &str = "3cadd1ee6394adea1bd021217a0e650ede09a323";

pub struct ModelFile {
    pub path: &'static str,
    pub sha256: &'static str,
    pub size: u64,
}

pub const MODEL_FILES: &[ModelFile] = &[
    ModelFile {
        path: "onnx/duration_predictor.onnx",
        sha256: "c3eb91414d5ff8a7a239b7fe9e34e7e2bf8a8140d8375ffb14718b1c639325db",
        size: 3700147,
    },
    ModelFile {
        path: "onnx/text_encoder.onnx",
        sha256: "c7befd5ea8c3119769e8a6c1486c4edc6a3bc8365c67621c881bbb774b9902ff",
        size: 36416150,
    },
    ModelFile {
        path: "onnx/vector_estimator.onnx",
        sha256: "883ac868ea0275ef0e991524dc64f16b3c0376efd7c320af6b53f5b780d7c61c",
        size: 256534781,
    },
    ModelFile {
        path: "onnx/vocoder.onnx",
        sha256: "085de76dd8e8d5836d6ca66826601f615939218f90e519f70ee8a36ed2a4c4ba",
        size: 101424195,
    },
    ModelFile {
        path: "onnx/tts.json",
        sha256: "42078d3aef1cd43ab43021f3c54f47d2d75ceb4e75f627f118890128b06a0d09",
        size: 8253,
    },
    ModelFile {
        path: "onnx/unicode_indexer.json",
        sha256: "9bf7346e43883a81f8645c81224f786d43c5b57f3641f6e7671a7d6c493cb24f",
        size: 277676,
    },
    ModelFile {
        path: "voice_styles/M1.json",
        sha256: "e35604687f5d23694b8e91593a93eec0e4eca6c0b02bb8ed69139ab2ea6b0a5b",
        size: 291748,
    },
    ModelFile {
        path: "voice_styles/M2.json",
        sha256: "b76cbf62bac707c710cf0ae5aba5e31eea1a6339a9734bfae33ab98499534a50",
        size: 292055,
    },
    ModelFile {
        path: "voice_styles/M3.json",
        sha256: "ea1ac35ccb91b0d7ecad533a2fbd0eec10c91513d8951e3b25fbba99954e159b",
        size: 290198,
    },
    ModelFile {
        path: "voice_styles/M4.json",
        sha256: "ca8eefad4fcd989c9379032ff3e50738adc547eeb5e221b82593a6d7b3bac303",
        size: 291522,
    },
    ModelFile {
        path: "voice_styles/M5.json",
        sha256: "dd22b92740314321f8ae11c5e87f8dd60d060f15dd3a632b5adf77f471f77af2",
        size: 291469,
    },
    ModelFile {
        path: "voice_styles/F1.json",
        sha256: "bbdec6ee00231c2c742ad05483df5334cab3b52fda3ba38e6a07059c4563dbc2",
        size: 292046,
    },
    ModelFile {
        path: "voice_styles/F2.json",
        sha256: "7c722c6a72707b1a77f035d67f0d1351ba187738e06f7683e8c72b1df3477fc6",
        size: 292423,
    },
    ModelFile {
        path: "voice_styles/F3.json",
        sha256: "12f6ef2573baa2defa1128069cb59f203e3ab67c92af77b42df8a0e3a2f7c6ab",
        size: 290794,
    },
    ModelFile {
        path: "voice_styles/F4.json",
        sha256: "c2fa764c1225a76dfc3e2c73e8aa4f70d9ee48793860eb34c295fff01c2e032b",
        size: 291808,
    },
    ModelFile {
        path: "voice_styles/F5.json",
        sha256: "45966e73316415626cf41a7d1c6f3b4c70dbc1ba2bee5c1978ef0ce33244fc8d",
        size: 291479,
    },
    ModelFile {
        path: "LICENSE",
        sha256: "0d944a9110fed9a9602d60e0423a272903e7bd21ab060490774efc77c2275e9f",
        size: 15007,
    },
];

pub const SPEAKER_ENCODER: ModelFile = ModelFile {
    path: "speaker_encoder.onnx",
    sha256: "c59158379255ad66e161679cca6af8d52d51e389e3224ab7d7a7baae295c2db5",
    size: 26485263,
};

pub const SPEAKER_ENCODER_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/3dspeaker_speech_eres2net_sv_en_voxceleb_16k.onnx";

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{VlError, err, store::Store, style::read_style_file};

pub fn model_dir(root: &Path) -> PathBuf {
    root.join("models/supertonic-3").join(MODEL_REVISION)
}

pub fn encoder_path(root: &Path) -> PathBuf {
    root.join("models/speaker-encoder/speaker_encoder.onnx")
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatus {
    pub installed: bool,
    pub encoder_installed: bool,
    pub ffmpeg: bool,
}

pub fn status(root: &Path) -> ModelStatus {
    let matches = |path: PathBuf, size| {
        path.metadata()
            .is_ok_and(|m| m.is_file() && m.len() == size)
    };
    ModelStatus {
        installed: MODEL_FILES
            .iter()
            .all(|f| matches(model_dir(root).join(f.path), f.size)),
        encoder_installed: matches(encoder_path(root), SPEAKER_ENCODER.size),
        ffmpeg: ffmpeg_available(),
    }
}

pub async fn install_files(
    files: &[(String, PathBuf, &'static str, u64)],
    tmp: &Path,
    progress: &(dyn Fn(u64, u64) + Send + Sync),
) -> Result<(), VlError> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| err("MODEL_DOWNLOAD_FAILED"))?;
    tokio::fs::create_dir_all(tmp).await?;
    let total = files.iter().map(|f| f.3).sum();
    let mut done = 0;
    progress(done, total);
    for (url, dest, expected, size) in files {
        // Hash existing files in bounded chunks as well as downloads.
        let mut hash = Sha256::new();
        let valid = if let Ok(mut file) = tokio::fs::File::open(dest).await {
            let mut buf = [0; 65536];
            loop {
                let n = file.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                hash.update(&buf[..n]);
            }
            format!("{:x}", hash.finalize()) == *expected
        } else {
            false
        };
        if valid {
            done += size;
            progress(done, total);
            continue;
        }
        let part = tmp.join(format!("{expected}.part"));
        let result: Result<(), VlError> = async {
            let mut response = client
                .get(url)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .map_err(|_| err("MODEL_DOWNLOAD_FAILED"))?;
            let mut file = tokio::fs::File::create(&part).await?;
            let mut hash = Sha256::new();
            let mut received = 0;
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| err("MODEL_DOWNLOAD_FAILED"))?
            {
                hash.update(&chunk);
                file.write_all(&chunk).await?;
                received += chunk.len() as u64;
                progress(done + received.min(*size), total);
            }
            file.flush().await?;
            drop(file);
            if format!("{:x}", hash.finalize()) != *expected {
                return Err(err("MODEL_CHECKSUM_MISMATCH"));
            }
            if let Some(parent) = dest.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::rename(&part, dest).await?;
            Ok(())
        }
        .await;
        if let Err(error) = result {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(error);
        }
        done += size;
        progress(done, total);
    }
    Ok(())
}

pub async fn install(
    root: &Path,
    with_encoder: bool,
    progress: &(dyn Fn(u64, u64) + Send + Sync),
) -> Result<(), VlError> {
    let mut files: Vec<_> = MODEL_FILES
        .iter()
        .map(|file| {
            (
                format!(
                    "https://huggingface.co/Supertone/supertonic-3/resolve/{MODEL_REVISION}/{}",
                    file.path
                ),
                model_dir(root).join(file.path),
                file.sha256,
                file.size,
            )
        })
        .collect();
    if with_encoder {
        files.push((
            SPEAKER_ENCODER_URL.to_string(),
            encoder_path(root),
            SPEAKER_ENCODER.sha256,
            SPEAKER_ENCODER.size,
        ));
    }
    install_files(&files, &root.join("tmp"), progress).await
}

pub fn register_builtins(store: &Store, root: &Path) -> Result<(), VlError> {
    for name in ["M1", "M2", "M3", "M4", "M5", "F1", "F2", "F3", "F4", "F5"] {
        let style = read_style_file(&model_dir(root).join(format!("voice_styles/{name}.json")))?;
        let profile = store.put_profile(&style, name, "builtin", None, None)?;
        if profile.origin != "builtin" {
            store.mark_builtin(profile.id, name)?;
        }
    }
    Ok(())
}

pub fn ffmpeg_available() -> bool {
    let mut command = Command::new("ffmpeg");
    command
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command.status().is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice_lab::{
        store::Store,
        style::{canonical_bytes, sha256_hex},
    };
    use ndarray::Array3;
    use std::{fs, sync::Mutex};
    use supertonic3_tts::Style;
    use tempfile::tempdir;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    // Fixed SHA-256 of "abc".
    const SHA: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[tokio::test]
    async fn install_downloads_and_verifies() {
        let server = MockServer::start().await;
        for route in ["/a", "/b"] {
            Mock::given(method("GET"))
                .and(path(route))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(b"abc"))
                .expect(1)
                .mount(&server)
                .await;
        }
        let dir = tempdir().unwrap();
        let files = vec![
            (
                format!("{}/a", server.uri()),
                dir.path().join("models/a"),
                SHA,
                3,
            ),
            (
                format!("{}/b", server.uri()),
                dir.path().join("models/onnx/b"),
                SHA,
                3,
            ),
        ];
        let events = Mutex::new(Vec::new());
        install_files(&files, &dir.path().join("tmp"), &|done, total| {
            events.lock().unwrap().push((done, total))
        })
        .await
        .unwrap();
        for (_, dest, _, _) in &files {
            assert_eq!(fs::read(dest).unwrap(), b"abc");
        }
        let events = events.lock().unwrap();
        assert_eq!(events.last(), Some(&(6, 6)));
        assert!(
            events
                .iter()
                .all(|&(done, total)| total == 6 && done <= total)
        );
        assert!(dir.path().join("tmp").read_dir().unwrap().next().is_none());
    }

    #[tokio::test]
    async fn checksum_mismatch_not_installed() {
        let server = MockServer::start().await;
        Mock::given(path("/a"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"bad"))
            .mount(&server)
            .await;
        let dir = tempdir().unwrap();
        let dest = dir.path().join("models/a");
        let tmp = dir.path().join("tmp");
        let error = install_files(
            &[(format!("{}/a", server.uri()), dest.clone(), SHA, 3)],
            &tmp,
            &|_, _| {},
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), "MODEL_CHECKSUM_MISMATCH");
        assert!(!dest.exists());
        assert!(!tmp.join(format!("{SHA}.part")).exists());
    }

    #[tokio::test]
    async fn install_skips_valid_files() {
        let server = MockServer::start().await;
        Mock::given(path("/a"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;
        let dir = tempdir().unwrap();
        let dest = dir.path().join("a");
        fs::write(&dest, b"abc").unwrap();
        let events = Mutex::new(Vec::new());
        install_files(
            &[(format!("{}/a", server.uri()), dest, SHA, 3)],
            &dir.path().join("tmp"),
            &|d, t| events.lock().unwrap().push((d, t)),
        )
        .await
        .unwrap();
        assert_eq!(events.lock().unwrap().last(), Some(&(3, 3)));
    }

    #[tokio::test]
    async fn http_error_is_download_failed() {
        let server = MockServer::start().await;
        Mock::given(path("/a"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let dir = tempdir().unwrap();
        let dest = dir.path().join("a");
        let tmp = dir.path().join("tmp");
        assert_eq!(
            install_files(
                &[(format!("{}/a", server.uri()), dest.clone(), SHA, 3)],
                &tmp,
                &|_, _| {}
            )
            .await
            .unwrap_err()
            .code(),
            "MODEL_DOWNLOAD_FAILED"
        );
        assert!(!dest.exists());
        assert!(!tmp.join(format!("{SHA}.part")).exists());
    }

    #[test]
    fn status_reports_missing() {
        let dir = tempdir().unwrap();
        let s = status(dir.path());
        assert!(!s.installed);
        assert!(!s.encoder_installed);
        assert_eq!(
            model_dir(dir.path()),
            dir.path().join("models/supertonic-3").join(MODEL_REVISION)
        );
        assert_eq!(
            encoder_path(dir.path()),
            dir.path()
                .join("models/speaker-encoder/speaker_encoder.onnx")
        );
        // Sparse files exercise the size-only status check without downloading models.
        for file in MODEL_FILES {
            let dest = model_dir(dir.path()).join(file.path);
            fs::create_dir_all(dest.parent().unwrap()).unwrap();
            fs::File::create(dest).unwrap().set_len(file.size).unwrap();
        }
        let encoder = encoder_path(dir.path());
        fs::create_dir_all(encoder.parent().unwrap()).unwrap();
        fs::File::create(&encoder)
            .unwrap()
            .set_len(SPEAKER_ENCODER.size)
            .unwrap();
        let s = status(dir.path());
        assert!(s.installed && s.encoder_installed);
        fs::File::create(model_dir(dir.path()).join(MODEL_FILES[0].path)).unwrap();
        fs::File::create(encoder).unwrap();
        let s = status(dir.path());
        assert!(!s.installed && !s.encoder_installed);
    }

    #[test]
    fn register_builtins_from_dir() {
        let dir = tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let styles = model_dir(dir.path()).join("voice_styles");
        fs::create_dir_all(&styles).unwrap();
        let mut imported_id = 0;
        for (i, name) in ["M1", "M2", "M3", "M4", "M5", "F1", "F2", "F3", "F4", "F5"]
            .iter()
            .enumerate()
        {
            let style = Style {
                ttl: Array3::from_elem((1, 50, 256), i as f32),
                dp: Array3::from_elem((1, 8, 16), 0.5),
            };
            let bytes = canonical_bytes(&style);
            assert_eq!(sha256_hex(&bytes).len(), 64);
            fs::write(styles.join(format!("{name}.json")), bytes).unwrap();
            if i == 0 {
                imported_id = store
                    .put_profile(&style, "Imported", "import", None, None)
                    .unwrap()
                    .id;
            }
        }
        register_builtins(&store, dir.path()).unwrap();
        register_builtins(&store, dir.path()).unwrap();
        let profiles = store.profiles().unwrap();
        assert_eq!(profiles.len(), 10);
        assert!(profiles.iter().all(|p| p.origin == "builtin"));
        assert_eq!(store.profile(imported_id).unwrap().name, "M1");
        assert_eq!(
            store.delete_profile(imported_id).unwrap_err().code(),
            "BUILTIN_LOCKED"
        );
        assert_eq!(
            store.mark_builtin(-1, "missing").unwrap_err().code(),
            "NOT_FOUND"
        );
    }
}
