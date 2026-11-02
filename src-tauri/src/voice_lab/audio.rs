use super::{
    VlError, err,
    store::{Source, Store},
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Cursor, Read},
    path::Path,
    process::{Command, Stdio},
    sync::Mutex,
};
use supertonic3_tts::features;

pub const SR: u32 = 44100;
const MAX_BYTES: u64 = 250 * 1024 * 1024;
const MAX_MS: u64 = 1_800_000;

fn check_size(path: &Path) -> Result<(), VlError> {
    if fs::metadata(path)?.len() > MAX_BYTES {
        return Err(err("AUDIO_TOO_LARGE"));
    }
    Ok(())
}

pub fn decode_to_wav(src: &Path, dst: &Path, ffmpeg: bool) -> Result<u64, VlError> {
    check_size(src)?;
    let result = (|| {
        if ffmpeg {
            let mut command = Command::new("ffmpeg");
            command
                .args([
                    "-nostdin",
                    "-v",
                    "error",
                    "-protocol_whitelist",
                    "file",
                    "-i",
                ])
                .arg(src)
                .args([
                    "-map",
                    "0:a:0",
                    "-t",
                    "1801",
                    "-ac",
                    "1",
                    "-ar",
                    "44100",
                    "-c:a",
                    "pcm_s16le",
                    "-f",
                    "wav",
                    "-y",
                ])
                .arg(dst)
                .stderr(Stdio::null())
                .stdout(Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x08000000);
            }
            if !command
                .status()
                .map_err(|_| err("AUDIO_UNSUPPORTED"))?
                .success()
            {
                return Err(err("AUDIO_UNSUPPORTED"));
            }
        } else {
            if !src
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("wav"))
            {
                return Err(err("AUDIO_UNSUPPORTED"));
            }
            let reader = hound::WavReader::open(src).map_err(|_| err("AUDIO_UNSUPPORTED"))?;
            let spec = reader.spec();
            if spec.sample_rate == 0 || spec.channels == 0 {
                return Err(err("AUDIO_UNSUPPORTED"));
            }
            if reader.duration() as u64 * 1000 > MAX_MS * spec.sample_rate as u64 {
                return Err(err("AUDIO_TOO_LONG"));
            }
            let audio = features::load_wav(src).map_err(|_| err("AUDIO_UNSUPPORTED"))?;
            let samples = features::resample(&audio.samples, audio.sample_rate, SR);
            fs::write(dst, wav_bytes(&samples, SR))?;
        }
        let reader = hound::WavReader::open(dst).map_err(|_| err("AUDIO_UNSUPPORTED"))?;
        if reader.duration() as u64 * 1000 > MAX_MS * SR as u64 {
            return Err(err("AUDIO_TOO_LONG"));
        }
        Ok(reader.duration() as u64 * 1000 / SR as u64)
    })();
    if result.is_err() {
        let _ = fs::remove_file(dst);
    }
    result
}

pub fn import(
    store: &Mutex<Store>,
    path: &Path,
    consent: bool,
    ffmpeg: bool,
) -> Result<Source, VlError> {
    if !path.is_absolute() {
        return Err(err("AUDIO_UNSUPPORTED"));
    }
    check_size(path)?;
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    let sha = format!("{:x}", hash.finalize());
    let root = {
        let s = store.lock().map_err(|_| err("IO_FAILED"))?;
        if let Some(source) = s.find_source_by_sha(&sha)? {
            return Ok(source);
        }
        s.root().to_path_buf()
    };
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("bin")
        .to_ascii_lowercase();
    let ext = if (1..=5).contains(&ext.len())
        && ext
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    {
        ext
    } else {
        "bin".into()
    };
    // Unique staging files allow concurrent imports without clobbering each other.
    let token = rand::random::<u64>();
    let tmp = root.join(format!("tmp/{sha}-{token:x}.wav"));
    let copy = root.join(format!("tmp/{sha}-{token:x}.original.{ext}"));
    let original = root.join(format!("sources/original/{sha}.{ext}"));
    let result = (|| {
        fs::copy(path, &copy)?;
        let duration = decode_to_wav(&copy, &tmp, ffmpeg)?;
        let values = peaks(&load_mono(&tmp)?, 1000);
        let s = store.lock().map_err(|_| err("IO_FAILED"))?;
        if let Some(source) = s.find_source_by_sha(&sha)? {
            return Ok(source);
        }
        fs::rename(&copy, &original)?;
        let source = match s.add_source(
            &sha,
            &ext,
            &path.file_name().unwrap_or_default().to_string_lossy(),
            duration as i64,
            consent,
            &values,
        ) {
            Ok(source) => source,
            Err(e) => {
                let _ = fs::remove_file(&original);
                return Err(e);
            }
        };
        if let Err(e) = fs::rename(&tmp, s.source_work_path(source.id)) {
            let _ = fs::remove_file(&original);
            let _ = s.rollback_source(source.id);
            return Err(e.into());
        }
        Ok(source)
    })();
    let _ = fs::remove_file(tmp);
    let _ = fs::remove_file(copy);
    result
}

pub fn load_mono(wav: &Path) -> Result<Vec<f32>, VlError> {
    features::load_wav(wav)
        .map(|a| a.samples)
        .map_err(|_| err("AUDIO_UNSUPPORTED"))
}

/// Reads only the requested fragment of the normalized mono PCM16 work WAV.
pub fn read_segment(path: &Path, start_ms: u64, end_ms: u64) -> Result<Vec<f32>, VlError> {
    let mut reader = hound::WavReader::open(path).map_err(|_| err("IO_FAILED"))?;
    let index = |ms: u64| {
        (u128::from(ms) * u128::from(SR) / 1000).min(u128::from(reader.duration())) as u32
    };
    let start = index(start_ms);
    let end = index(end_ms).max(start);
    reader.seek(start).map_err(|_| err("IO_FAILED"))?;
    reader
        .samples::<i16>()
        .take((end - start) as usize)
        .map(|s| s.map(|v| v as f32 / 32768.0))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| err("IO_FAILED"))
}

pub fn peaks(samples: &[f32], n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let len = samples.len() as u64;
            let start = (i as u64 * len / n as u64) as usize;
            let end = ((i as u64 + 1) * len / n as u64) as usize;
            samples[start..end]
                .iter()
                .fold(0.0f32, |p, x| p.max(x.abs()))
                .clamp(0.0, 1.0)
        })
        .collect()
}

pub fn segment(samples: &[f32], start_ms: u64, end_ms: u64) -> &[f32] {
    let index = |ms: u64| ((ms as u128 * SR as u128 / 1000).min(samples.len() as u128)) as usize;
    let start = index(start_ms);
    let end = index(end_ms).max(start);
    &samples[start..end]
}

pub fn validate_range(
    duration_ms: u64,
    start_ms: u64,
    end_ms: u64,
    min_ms: u64,
    max_ms: u64,
) -> Result<(), VlError> {
    if start_ms >= end_ms
        || end_ms > duration_ms
        || !(min_ms..=max_ms).contains(&(end_ms - start_ms))
    {
        return Err(err("REGION_INVALID"));
    }
    Ok(())
}

pub fn wav_bytes(samples: &[f32], sr: u32) -> Vec<u8> {
    let mut cursor = Cursor::new(Vec::new());
    let mut writer = hound::WavWriter::new(
        &mut cursor,
        hound::WavSpec {
            channels: 1,
            sample_rate: sr,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .expect("valid mono WAV spec");
    for sample in samples {
        writer
            .write_sample((sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16)
            .expect("writing to memory");
    }
    writer.finalize().expect("finalizing in memory");
    cursor.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn fixture(path: &Path, channels: u16, rate: u32, seconds: u32) {
        let mut w = hound::WavWriter::create(
            path,
            hound::WavSpec {
                channels,
                sample_rate: rate,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for i in 0..rate * seconds {
            for _ in 0..channels {
                w.write_sample(((i as f32 * 0.06).sin() * 16000.0) as i16)
                    .unwrap();
            }
        }
        w.finalize().unwrap();
    }
    #[test]
    fn stereo_48k_int16_becomes_mono_44k() {
        let d = tempdir().unwrap();
        let src = d.path().join("in.wav");
        let dst = d.path().join("out.wav");
        fixture(&src, 2, 48000, 2);
        assert!(decode_to_wav(&src, &dst, false).unwrap().abs_diff(2000) <= 5);
        let r = hound::WavReader::open(&dst).unwrap();
        assert_eq!(
            (
                r.spec().channels,
                r.spec().sample_rate,
                r.spec().bits_per_sample
            ),
            (1, SR, 16)
        );
        assert!(load_mono(&dst).unwrap().iter().any(|s| s.abs() > 0.1));
    }
    #[test]
    fn mp3_without_ffmpeg_unsupported() {
        let d = tempdir().unwrap();
        let p = d.path().join("a.mp3");
        fs::write(&p, b"fake").unwrap();
        assert_eq!(
            decode_to_wav(&p, &d.path().join("out.wav"), false)
                .unwrap_err()
                .code(),
            "AUDIO_UNSUPPORTED"
        );
    }
    #[test]
    fn garbage_wav_unsupported() {
        let d = tempdir().unwrap();
        let p = d.path().join("bad.wav");
        fs::write(&p, b"bad").unwrap();
        let store = Mutex::new(Store::open(&d.path().join("lab")).unwrap());
        assert_eq!(
            import(&store, &p, true, false).unwrap_err().code(),
            "AUDIO_UNSUPPORTED"
        );
        let s = store.lock().unwrap();
        assert!(s.sources().unwrap().is_empty());
        for sub in ["tmp", "sources/work", "sources/original"] {
            assert_eq!(fs::read_dir(s.root().join(sub)).unwrap().count(), 0);
        }
    }
    #[test]
    fn relative_path_rejected() {
        let d = tempdir().unwrap();
        let s = Mutex::new(Store::open(d.path()).unwrap());
        assert_eq!(
            import(&s, Path::new("a.wav"), true, false)
                .unwrap_err()
                .code(),
            "AUDIO_UNSUPPORTED"
        );
    }
    #[test]
    fn range_bounds() {
        validate_range(5000, 0, 1000, 1000, 15000).unwrap();
        for (d, s, e) in [
            (5000, 1000, 1000),
            (5000, 0, 5001),
            (5000, 0, 999),
            (60000, 0, 15001),
            (5000, u64::MAX, 0),
        ] {
            assert_eq!(
                validate_range(d, s, e, 1000, 15000).unwrap_err().code(),
                "REGION_INVALID"
            );
        }
    }
    #[test]
    fn peaks_len_and_range() {
        let p = peaks(&[-2.0, 0.5, 0.0, 1.5], 1000);
        assert_eq!(p.len(), 1000);
        assert!(p.iter().all(|x| (0.0..=1.0).contains(x)));
        assert!(p.contains(&1.0));
        assert_eq!(peaks(&[], 3), vec![0.0; 3]);
        assert!(peaks(&[1.0], 0).is_empty());
    }
    #[test]
    fn segment_length() {
        let s = vec![0.0; SR as usize * 2];
        assert_eq!(segment(&s, 500, 1500).len(), 44100);
        let bytes = wav_bytes(&s, SR);
        let r = hound::WavReader::new(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(r.duration(), SR * 2);
    }
    #[test]
    fn read_segment_seeks_and_bounds_the_read() {
        let d = tempdir().unwrap();
        let path = d.path().join("work.wav");
        let samples = [vec![0.25; SR as usize], vec![-0.5; SR as usize]].concat();
        fs::write(&path, wav_bytes(&samples, SR)).unwrap();
        let fragment = read_segment(&path, 1000, 1500).unwrap();
        assert_eq!(fragment.len(), SR as usize / 2);
        assert!(fragment.iter().all(|s| (*s + 0.5).abs() < 0.0001));
        assert_eq!(
            read_segment(&path, 1500, u64::MAX).unwrap().len(),
            SR as usize / 2
        );
        assert!(read_segment(&path, u64::MAX, u64::MAX).unwrap().is_empty());
    }
    #[test]
    fn import_is_idempotent() {
        let d = tempdir().unwrap();
        let p = d.path().join("a.WAV");
        fixture(&p, 1, SR, 1);
        let store = Mutex::new(Store::open(&d.path().join("lab")).unwrap());
        let a = import(&store, &p, true, false).unwrap();
        let b = import(&store, &p, false, false).unwrap();
        assert_eq!(a.id, b.id);
        assert!(b.consent);
        let s = store.lock().unwrap();
        assert_eq!(s.sources().unwrap().len(), 1);
        assert!(s.source_work_path(a.id).exists());
        assert_eq!(s.source_peaks(a.id).unwrap().len(), 1000);
        assert_eq!(
            fs::read_dir(s.root().join("sources/original"))
                .unwrap()
                .count(),
            1
        );
    }
    #[test]
    fn rename_failure_rolls_back_source() {
        let d = tempdir().unwrap();
        let p = d.path().join("a.wav");
        fixture(&p, 1, SR, 1);
        let store = Mutex::new(Store::open(&d.path().join("lab")).unwrap());
        fs::create_dir(store.lock().unwrap().source_work_path(1)).unwrap();
        assert!(import(&store, &p, true, false).is_err());
        let s = store.lock().unwrap();
        assert!(s.sources().unwrap().is_empty());
        assert_eq!(fs::read_dir(s.root().join("tmp")).unwrap().count(), 0);
        assert_eq!(
            fs::read_dir(s.root().join("sources/original"))
                .unwrap()
                .count(),
            0
        );
    }
    #[test]
    fn oversized_audio_rejected() {
        let d = tempdir().unwrap();
        let p = d.path().join("huge.wav");
        fs::File::create(&p)
            .unwrap()
            .set_len(250 * 1024 * 1024 + 1)
            .unwrap();
        assert_eq!(
            decode_to_wav(&p, &d.path().join("out.wav"), false)
                .unwrap_err()
                .code(),
            "AUDIO_TOO_LARGE"
        );
    }
    #[test]
    #[ignore = "requires ffmpeg"]
    fn ffmpeg_decodes_flac() {
        let d = tempdir().unwrap();
        let p = d.path().join("a.flac");
        assert!(
            Command::new("ffmpeg")
                .args([
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:duration=2"
                ])
                .arg(&p)
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            decode_to_wav(&p, &d.path().join("out.wav"), true).unwrap(),
            2000
        );
    }
    #[test]
    #[ignore = "requires ffmpeg"]
    fn ffmpeg_caps_long_input() {
        let d = tempdir().unwrap();
        let p = d.path().join("long.flac");
        let out = d.path().join("out.wav");
        assert!(
            Command::new("ffmpeg")
                .args([
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:sample_rate=8000:duration=1860"
                ])
                .arg(&p)
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            decode_to_wav(&p, &out, true).unwrap_err().code(),
            "AUDIO_TOO_LONG"
        );
        assert!(!out.exists());
    }
}
