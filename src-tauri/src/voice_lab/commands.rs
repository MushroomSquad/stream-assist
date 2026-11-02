use super::{
    VlError, VlState, VoiceLab, audio, err,
    mixer::{self, DpStrategy, Recipe, RecipeInput, new_recipe},
    models,
    runner::{Emit, JobState},
    store::{Profile, Render, RenderParams, Source},
    style::{canonical_bytes, load_verified, read_style_file},
    worker::ClonePreset,
};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use supertonic3_tts::{AVAILABLE_LANGS, Style};
use tauri::{Emitter, State, ipc::Response};

fn lab(s: &VlState) -> Result<Arc<VoiceLab>, String> {
    s.0.clone().ok_or_else(|| "VOICE_LAB_UNAVAILABLE".into())
}

async fn blocking<T: Send + 'static>(
    lab: Arc<VoiceLab>,
    f: impl FnOnce(&VoiceLab) -> Result<T, VlError> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(move || f(&lab))
        .await
        .map_err(|_| "IO_FAILED".to_string())?
        .map_err(|e| e.code())
}

fn require_model(root: &Path) -> Result<(), VlError> {
    if models::installed(root) {
        Ok(())
    } else {
        Err(err("MODEL_MISSING"))
    }
}

fn validate_params(p: &RenderParams) -> Result<(), VlError> {
    if !(1..=1000).contains(&p.text.trim().chars().count()) {
        return Err(err("TEXT_INVALID"));
    }
    if !AVAILABLE_LANGS.contains(&p.lang.as_str())
        || !p.speed.is_finite()
        || !(0.7..=1.5).contains(&p.speed)
        || !(5..=12).contains(&p.quality)
    {
        return Err(err("RENDER_INVALID"));
    }
    Ok(())
}

fn build_mix(
    lab: &VoiceLab,
    inputs: Vec<(i64, f64)>,
    dp: DpStrategy,
    select_id: Option<i64>,
) -> Result<(Style, Recipe), VlError> {
    let mut ids = HashSet::new();
    if !(1..=8).contains(&inputs.len())
        || inputs.iter().any(|(id, _)| !ids.insert(*id))
        || select_id.is_some_and(|id| !ids.contains(&id))
    {
        return Err(err("MIX_INVALID"));
    }
    let select_index = select_id.and_then(|id| inputs.iter().position(|(i, _)| *i == id));
    let s = lab.store.lock().map_err(|_| err("IO_FAILED"))?;
    let mut paths = Vec::new();
    let mut recipe_inputs = Vec::new();
    for (id, weight) in &inputs {
        let profile = s.profile(*id)?;
        paths.push((s.profile_path(&profile), profile.style_sha256.clone()));
        recipe_inputs.push(RecipeInput {
            profile_sha256: profile.style_sha256,
            name: profile.name,
            weight: *weight,
        });
    }
    drop(s);
    let styles = paths
        .iter()
        .map(|(path, sha)| load_verified(path, sha))
        .collect::<Result<Vec<_>, _>>()?;
    let refs: Vec<_> = styles
        .iter()
        .zip(&inputs)
        .map(|(s, (_, w))| (s, *w))
        .collect();
    let style = mixer::mix(&refs, dp, select_index)?;
    let selected = select_index.map(|i| recipe_inputs[i].profile_sha256.clone());
    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(anyhow::Error::from)?
        .as_secs()
        .to_string();
    Ok((style, new_recipe(recipe_inputs, dp, selected, created_at)))
}

pub(crate) fn save_snapshot(
    lab: &VoiceLab,
    inputs: Vec<(i64, f64)>,
    dp: DpStrategy,
    select_id: Option<i64>,
    name: String,
) -> Result<Profile, VlError> {
    let (style, recipe) = build_mix(lab, inputs, dp, select_id)?;
    lab.store.lock().map_err(|_| err("IO_FAILED"))?.put_profile(
        &style,
        &name,
        "mix",
        Some(&recipe),
        None,
    )
}

pub(crate) async fn render(
    lab: &VoiceLab,
    profile_id: i64,
    params: RenderParams,
    emit: Emit,
) -> Result<Render, VlError> {
    validate_params(&params)?;
    let path = {
        let s = lab.store.lock().map_err(|_| err("IO_FAILED"))?;
        s.load_style(profile_id)?;
        s.profile_path(&s.profile(profile_id)?)
    };
    lab.runner
        .synth(
            &lab.store,
            &lab.root,
            path,
            Some(profile_id),
            None,
            params,
            emit,
        )
        .await
}

pub(crate) async fn mix_preview(
    lab: &VoiceLab,
    inputs: Vec<(i64, f64)>,
    dp: DpStrategy,
    select_id: Option<i64>,
    params: RenderParams,
    emit: Emit,
) -> Result<Render, VlError> {
    validate_params(&params)?;
    let (style, recipe) = build_mix(lab, inputs, dp, select_id)?;
    let path = lab
        .root
        .join(format!("tmp/mix-{:x}.json", rand::random::<u64>()));
    // Also clean up when the future is dropped or the worker fails.
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _cleanup = Cleanup(path.clone());
    std::fs::write(&path, canonical_bytes(&style))?;
    lab.runner
        .synth(
            &lab.store,
            &lab.root,
            path,
            None,
            Some(recipe),
            params,
            emit,
        )
        .await
}

fn export_bytes(path: &Path, ext: &str, bytes: &[u8]) -> Result<(), VlError> {
    if !path.is_absolute()
        || !path.parent().is_some_and(Path::is_dir)
        || path.extension().and_then(|s| s.to_str()) != Some(ext)
    {
        return Err(err("EXPORT_FAILED"));
    }
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let result = (|| {
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, path)
    })();
    let _ = std::fs::remove_file(tmp);
    result.map_err(|_| err("EXPORT_FAILED"))
}

fn job_emitter(app: tauri::AppHandle) -> Emit {
    Arc::new(move |job| {
        let _ = app.emit("vl-job", job);
    })
}

#[tauri::command]
pub async fn vl_status(state: State<'_, VlState>) -> Result<models::ModelStatus, String> {
    blocking(lab(&state)?, |l| Ok(models::status(&l.root))).await
}

#[tauri::command]
pub async fn vl_install(
    state: State<'_, VlState>,
    app: tauri::AppHandle,
    with_encoder: bool,
) -> Result<(), String> {
    let l = lab(&state)?;
    let _guard = l
        .installing
        .try_lock()
        .map_err(|_| "JOB_BUSY".to_string())?;
    let last = std::sync::Mutex::new(None::<Instant>);
    let progress = |done, total| {
        let mut last = last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if done == total || last.is_none_or(|t| t.elapsed() >= Duration::from_millis(200)) {
            let _ = app.emit("vl-install", serde_json::json!({"done":done,"total":total}));
            *last = Some(Instant::now());
        }
    };
    models::install(&l.root, with_encoder, &progress)
        .await
        .map_err(|e| e.code())?;
    blocking(l.clone(), |l| {
        let store = l.store.lock().map_err(|_| err("IO_FAILED"))?;
        models::register_builtins(&store, &l.root)
    })
    .await
}

pub(crate) fn import_audio(
    lab: &VoiceLab,
    path: PathBuf,
    consent: bool,
) -> Result<Source, VlError> {
    audio::import(&lab.store, &path, consent, models::ffmpeg_available())
}

#[tauri::command]
pub async fn vl_import_audio(
    state: State<'_, VlState>,
    path: PathBuf,
    consent: bool,
) -> Result<Source, String> {
    blocking(lab(&state)?, move |l| import_audio(l, path, consent)).await
}

pub(crate) fn sources(lab: &VoiceLab) -> Result<Vec<Source>, VlError> {
    lab.store.lock().map_err(|_| err("IO_FAILED"))?.sources()
}

#[tauri::command]
pub async fn vl_sources(state: State<'_, VlState>) -> Result<Vec<Source>, String> {
    blocking(lab(&state)?, sources).await
}

pub(crate) fn set_consent(lab: &VoiceLab, id: i64, consent: bool) -> Result<(), VlError> {
    lab.store
        .lock()
        .map_err(|_| err("IO_FAILED"))?
        .set_consent(id, consent)
}

#[tauri::command]
pub async fn vl_set_consent(
    state: State<'_, VlState>,
    id: i64,
    consent: bool,
) -> Result<(), String> {
    blocking(lab(&state)?, move |l| set_consent(l, id, consent)).await
}

pub(crate) fn peaks(lab: &VoiceLab, source_id: i64) -> Result<Vec<f32>, VlError> {
    lab.store
        .lock()
        .map_err(|_| err("IO_FAILED"))?
        .source_peaks(source_id)
}

#[tauri::command]
pub async fn vl_peaks(state: State<'_, VlState>, source_id: i64) -> Result<Vec<f32>, String> {
    blocking(lab(&state)?, move |l| peaks(l, source_id)).await
}

pub(crate) fn source_audio(
    lab: &VoiceLab,
    source_id: i64,
    start_ms: u64,
    end_ms: u64,
) -> Result<Vec<u8>, VlError> {
    let (source, path) = {
        let s = lab.store.lock().map_err(|_| err("IO_FAILED"))?;
        (s.source(source_id)?, s.source_work_path(source_id))
    };
    audio::validate_range(source.duration_ms as u64, start_ms, end_ms, 1, 60000)?;
    let samples = audio::read_segment(&path, start_ms, end_ms)?;
    Ok(audio::wav_bytes(&samples, audio::SR))
}

#[tauri::command]
pub async fn vl_source_audio(
    state: State<'_, VlState>,
    source_id: i64,
    start_ms: u64,
    end_ms: u64,
) -> Result<Response, String> {
    blocking(lab(&state)?, move |l| {
        source_audio(l, source_id, start_ms, end_ms)
    })
    .await
    .map(Response::new)
}

pub(crate) fn cancel_job(lab: &VoiceLab, id: u64) -> Result<(), VlError> {
    lab.runner.cancel(id)
}

#[tauri::command]
pub async fn vl_cancel_job(state: State<'_, VlState>, id: u64) -> Result<(), String> {
    cancel_job(lab(&state)?.as_ref(), id).map_err(|e| e.code())
}

pub(crate) fn jobs(lab: &VoiceLab) -> Result<Vec<JobState>, VlError> {
    Ok(lab.runner.jobs())
}

#[tauri::command]
pub async fn vl_jobs(state: State<'_, VlState>) -> Result<Vec<JobState>, String> {
    jobs(lab(&state)?.as_ref()).map_err(|e| e.code())
}

pub(crate) fn import_profile(
    lab: &VoiceLab,
    path: PathBuf,
    name: String,
) -> Result<Profile, VlError> {
    if !path.is_absolute() {
        return Err(err("IO_FAILED"));
    }
    let style = read_style_file(&path)?;
    lab.store
        .lock()
        .map_err(|_| err("IO_FAILED"))?
        .put_profile(&style, &name, "import", None, None)
}

#[tauri::command]
pub async fn vl_import_profile(
    state: State<'_, VlState>,
    path: PathBuf,
    name: String,
) -> Result<Profile, String> {
    blocking(lab(&state)?, move |l| import_profile(l, path, name)).await
}

pub(crate) fn profiles(lab: &VoiceLab) -> Result<Vec<Profile>, VlError> {
    lab.store.lock().map_err(|_| err("IO_FAILED"))?.profiles()
}

#[tauri::command]
pub async fn vl_profiles(state: State<'_, VlState>) -> Result<Vec<Profile>, String> {
    blocking(lab(&state)?, profiles).await
}

pub(crate) fn rename_profile(
    lab: &VoiceLab,
    id: i64,
    name: String,
    tags: Vec<String>,
) -> Result<(), VlError> {
    lab.store
        .lock()
        .map_err(|_| err("IO_FAILED"))?
        .rename_profile(id, &name, &tags)
}

#[tauri::command]
pub async fn vl_rename_profile(
    state: State<'_, VlState>,
    id: i64,
    name: String,
    tags: Vec<String>,
) -> Result<(), String> {
    blocking(lab(&state)?, move |l| rename_profile(l, id, name, tags)).await
}

pub(crate) fn delete_profile(lab: &VoiceLab, id: i64) -> Result<(), VlError> {
    lab.store
        .lock()
        .map_err(|_| err("IO_FAILED"))?
        .delete_profile(id)
}

#[tauri::command]
pub async fn vl_delete_profile(state: State<'_, VlState>, id: i64) -> Result<(), String> {
    blocking(lab(&state)?, move |l| delete_profile(l, id)).await
}

pub(crate) fn renders(lab: &VoiceLab) -> Result<Vec<Render>, VlError> {
    lab.store.lock().map_err(|_| err("IO_FAILED"))?.renders(200)
}

#[tauri::command]
pub async fn vl_renders(state: State<'_, VlState>) -> Result<Vec<Render>, String> {
    blocking(lab(&state)?, renders).await
}

pub(crate) fn render_audio(lab: &VoiceLab, id: i64) -> Result<Vec<u8>, VlError> {
    let path = {
        let s = lab.store.lock().map_err(|_| err("IO_FAILED"))?;
        s.render(id)?;
        s.render_path(id)
    };
    std::fs::read(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            err("NOT_FOUND")
        } else {
            e.into()
        }
    })
}

#[tauri::command]
pub async fn vl_render_audio(state: State<'_, VlState>, id: i64) -> Result<Response, String> {
    blocking(lab(&state)?, move |l| render_audio(l, id))
        .await
        .map(Response::new)
}

pub(crate) fn set_favorite(lab: &VoiceLab, id: i64, fav: bool) -> Result<(), VlError> {
    lab.store
        .lock()
        .map_err(|_| err("IO_FAILED"))?
        .set_favorite(id, fav)
}

#[tauri::command]
pub async fn vl_set_favorite(state: State<'_, VlState>, id: i64, fav: bool) -> Result<(), String> {
    blocking(lab(&state)?, move |l| set_favorite(l, id, fav)).await
}

pub(crate) fn export_profile(lab: &VoiceLab, id: i64, path: PathBuf) -> Result<(), VlError> {
    let profile_path = {
        let s = lab.store.lock().map_err(|_| err("IO_FAILED"))?;
        s.load_style(id)?;
        s.profile_path(&s.profile(id)?)
    };
    export_bytes(&path, "json", &std::fs::read(profile_path)?)
}

#[tauri::command]
pub async fn vl_export_profile(
    state: State<'_, VlState>,
    id: i64,
    path: PathBuf,
) -> Result<(), String> {
    blocking(lab(&state)?, move |l| export_profile(l, id, path)).await
}

pub(crate) fn export_recipe(lab: &VoiceLab, profile_id: i64, path: PathBuf) -> Result<(), VlError> {
    let recipe = lab
        .store
        .lock()
        .map_err(|_| err("IO_FAILED"))?
        .profile(profile_id)?
        .recipe
        .ok_or_else(|| err("NOT_FOUND"))?;
    export_bytes(&path, "json", &serde_json::to_vec(&recipe)?)
}

#[tauri::command]
pub async fn vl_export_recipe(
    state: State<'_, VlState>,
    profile_id: i64,
    path: PathBuf,
) -> Result<(), String> {
    blocking(lab(&state)?, move |l| export_recipe(l, profile_id, path)).await
}

pub(crate) fn export_render(lab: &VoiceLab, id: i64, path: PathBuf) -> Result<(), VlError> {
    export_bytes(&path, "wav", &render_audio(lab, id)?)
}

#[tauri::command]
pub async fn vl_export_render(
    state: State<'_, VlState>,
    id: i64,
    path: PathBuf,
) -> Result<(), String> {
    blocking(lab(&state)?, move |l| export_render(l, id, path)).await
}

#[tauri::command]
pub async fn vl_save_snapshot(
    state: State<'_, VlState>,
    inputs: Vec<(i64, f64)>,
    dp: DpStrategy,
    select_id: Option<i64>,
    name: String,
) -> Result<Profile, String> {
    blocking(lab(&state)?, move |l| {
        save_snapshot(l, inputs, dp, select_id, name)
    })
    .await
}

#[tauri::command]
pub async fn vl_render(
    state: State<'_, VlState>,
    app: tauri::AppHandle,
    profile_id: i64,
    params: RenderParams,
) -> Result<Render, String> {
    let handle = tokio::runtime::Handle::current();
    blocking(lab(&state)?, move |l| {
        require_model(&l.root)?;
        handle.block_on(render(l, profile_id, params, job_emitter(app)))
    })
    .await
}

#[tauri::command]
pub async fn vl_mix_preview(
    state: State<'_, VlState>,
    app: tauri::AppHandle,
    inputs: Vec<(i64, f64)>,
    dp: DpStrategy,
    select_id: Option<i64>,
    params: RenderParams,
) -> Result<Render, String> {
    let handle = tokio::runtime::Handle::current();
    blocking(lab(&state)?, move |l| {
        require_model(&l.root)?;
        handle.block_on(mix_preview(
            l,
            inputs,
            dp,
            select_id,
            params,
            job_emitter(app),
        ))
    })
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn clone(
    lab: &VoiceLab,
    source_id: i64,
    start_ms: u64,
    end_ms: u64,
    transcript: String,
    lang: String,
    name: String,
    preset: ClonePreset,
    seed: u64,
    emit: Emit,
) -> Result<u64, VlError> {
    let encoder = models::encoder_path(&lab.root);
    lab.runner.start_clone(
        lab.store.clone(),
        lab.root.clone(),
        models::model_dir(&lab.root),
        encoder.is_file().then_some(encoder),
        source_id,
        start_ms,
        end_ms,
        transcript,
        lang,
        name,
        preset,
        seed,
        emit,
    )
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn vl_clone(
    state: State<'_, VlState>,
    app: tauri::AppHandle,
    source_id: i64,
    start_ms: u64,
    end_ms: u64,
    transcript: String,
    lang: String,
    name: String,
    preset: ClonePreset,
    seed: u64,
) -> Result<u64, String> {
    blocking(lab(&state)?, move |l| {
        require_model(&l.root)?;
        clone(
            l,
            source_id,
            start_ms,
            end_ms,
            transcript,
            lang,
            name,
            preset,
            seed,
            job_emitter(app),
        )
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::super::{runner::Runner, store::Store};
    use super::*;
    use ndarray::Array3;
    use tempfile::tempdir;

    fn setup() -> (tempfile::TempDir, VoiceLab, Profile) {
        let d = tempdir().unwrap();
        let store = Store::open(d.path()).unwrap();
        let style = Style {
            ttl: Array3::from_elem((1, 50, 256), 0.1),
            dp: Array3::from_elem((1, 8, 16), 0.2),
        };
        let p = store
            .put_profile(&style, "Test", "import", None, None)
            .unwrap();
        let exe = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("examples")
            .join(format!("vl_fake_worker{}", std::env::consts::EXE_SUFFIX));
        let lab = VoiceLab {
            root: d.path().into(),
            store: Arc::new(std::sync::Mutex::new(store)),
            runner: Arc::new(Runner::new(exe, vec![("VL_FAKE_MODE".into(), "ok".into())])),
            installing: tokio::sync::Mutex::new(()),
        };
        (d, lab, p)
    }
    fn params() -> RenderParams {
        RenderParams {
            text: "Привет".into(),
            lang: "ru".into(),
            speed: 1.0,
            quality: 8,
        }
    }
    #[test]
    fn export_path_rules() {
        let d = tempdir().unwrap();
        for p in [
            PathBuf::from("relative.json"),
            d.path().join("missing/a.json"),
            d.path().join("a.wav"),
        ] {
            assert_eq!(
                export_bytes(&p, "json", b"{}").unwrap_err().code(),
                "EXPORT_FAILED"
            );
        }
        let p = d.path().join("a.json");
        export_bytes(&p, "json", b"{}").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"{}");
        export_bytes(&p, "json", b"[]").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"[]");
        assert!(!d.path().join("a.json.tmp").exists());
    }
    #[test]
    fn build_mix_rejects_duplicates_and_foreign_select() {
        let (_d, lab, p) = setup();
        for (inputs, select) in [
            (vec![], None),
            ((1..=9).map(|id| (id, 1.0)).collect(), None),
            (vec![(p.id, 50.0), (p.id, 50.0)], None),
            (vec![(p.id, 100.0)], Some(p.id + 1)),
        ] {
            assert_eq!(
                build_mix(&lab, inputs, DpStrategy::Dominant, select)
                    .unwrap_err()
                    .code(),
                "MIX_INVALID"
            );
        }
    }
    #[test]
    fn snapshot_hash_mismatch() {
        let (_d, lab, p) = setup();
        let path = lab.store.lock().unwrap().profile_path(&p);
        std::fs::write(path, b"{}").unwrap();
        assert_eq!(
            save_snapshot(
                &lab,
                vec![(p.id, 100.0)],
                DpStrategy::Dominant,
                None,
                "Snapshot".into()
            )
            .unwrap_err()
            .code(),
            "PROFILE_HASH_MISMATCH"
        );
    }
    #[test]
    fn snapshot_single_input_returns_existing() {
        let (_d, lab, p) = setup();
        let result = save_snapshot(
            &lab,
            vec![(p.id, 100.0)],
            DpStrategy::Dominant,
            None,
            "Snapshot".into(),
        )
        .unwrap();
        assert_eq!(result.id, p.id);
        assert_eq!(lab.store.lock().unwrap().profiles().unwrap().len(), 1);
    }
    #[tokio::test]
    async fn mix_preview_not_in_library() {
        let (_d, lab, p) = setup();
        let r = mix_preview(
            &lab,
            vec![(p.id, 100.0)],
            DpStrategy::Dominant,
            None,
            params(),
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
        assert!(r.profile_id.is_none());
        assert_eq!(r.recipe.unwrap().inputs[0].profile_sha256, p.style_sha256);
        let s = lab.store.lock().unwrap();
        assert_eq!(s.profiles().unwrap().len(), 1);
        assert_eq!(s.renders(200).unwrap().len(), 1);
        assert!(s.render_path(r.id).exists());
        assert_eq!(std::fs::read_dir(lab.root.join("tmp")).unwrap().count(), 0);
    }
    #[test]
    fn validate_params_bounds() {
        let mut p = params();
        for text in [" ".into(), "я".repeat(1001)] {
            p.text = text;
            assert_eq!(validate_params(&p).unwrap_err().code(), "TEXT_INVALID");
        }
        p.text = "я".repeat(1000);
        validate_params(&p).unwrap();
        for speed in [0.69, 1.51, f32::NAN, f32::INFINITY] {
            p.speed = speed;
            assert_eq!(validate_params(&p).unwrap_err().code(), "RENDER_INVALID");
        }
        for speed in [0.7, 1.5] {
            p.speed = speed;
            validate_params(&p).unwrap();
        }
        for quality in [4, 13] {
            p.quality = quality;
            assert_eq!(validate_params(&p).unwrap_err().code(), "RENDER_INVALID");
        }
        for quality in [5, 12] {
            p.quality = quality;
            validate_params(&p).unwrap();
        }
        p.lang = "xx".into();
        assert_eq!(validate_params(&p).unwrap_err().code(), "RENDER_INVALID");
    }
    #[test]
    fn require_model_without_model() {
        assert_eq!(
            require_model(tempdir().unwrap().path()).unwrap_err().code(),
            "MODEL_MISSING"
        );
    }
}
