use super::{
    VlError, VlState, VoiceLab, audio, err,
    mixer::{self, DpStrategy, Recipe, RecipeInput, new_recipe},
    models,
    runner::{Emit, JobState, TempFiles, cut_reference},
    store::{Profile, Render, RenderParams, Source},
    store::{TrainParams, TrainRun},
    style::{canonical_bytes, load_verified, read_style_file},
    trainer,
    worker::{CloneDevice, ClonePreset},
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

pub(crate) fn validate_params(p: &RenderParams) -> Result<(), VlError> {
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

// Used by the TTS engine once it is wired in.
#[allow(dead_code)]
pub(crate) async fn tts_synth(
    lab: Arc<VoiceLab>,
    profile_id: i64,
    params: RenderParams,
    out: PathBuf,
) -> Result<i64, VlError> {
    validate_params(&params)?;
    require_model(&lab.root)?;
    let path = {
        let s = lab.store.lock().map_err(|_| err("IO_FAILED"))?;
        s.load_style(profile_id)?;
        s.profile_path(&s.profile(profile_id)?)
    };
    lab.runner.synth_file(&lab.root, path, params, &out).await
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

#[derive(serde::Deserialize)]
#[serde(
    tag = "mode",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum Explore {
    Interp {
        a: i64,
        b: i64,
        points: usize,
    },
    Random {
        ids: Vec<i64>,
        k: usize,
        seed: u32,
        dp: DpStrategy,
    },
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefRegion {
    pub source_id: i64,
    pub start_ms: u64,
    pub end_ms: u64,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExploreItem {
    pub render: Render,
    pub similarity: Option<f32>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExploreResult {
    pub items: Vec<ExploreItem>,
    pub cancelled: bool,
}

pub(crate) async fn explore(
    lab: &VoiceLab,
    explore: Explore,
    params: RenderParams,
    reference: Option<RefRegion>,
    emit: Emit,
) -> Result<ExploreResult, VlError> {
    require_model(&lab.root)?;
    if !(1..=300).contains(&params.text.trim().chars().count()) {
        return Err(err("BATCH_INVALID"));
    }
    let (ids, weights, dp) = match explore {
        Explore::Interp { a, b, points } => {
            if !(3..=9).contains(&points) {
                return Err(err("BATCH_INVALID"));
            }
            (
                vec![a, b],
                mixer::interp_weights(points)
                    .into_iter()
                    .map(Vec::from)
                    .collect::<Vec<_>>(),
                DpStrategy::Weighted,
            )
        }
        Explore::Random { ids, k, seed, dp } => {
            if !(2..=8).contains(&ids.len()) || !(2..=8).contains(&k) || dp == DpStrategy::Select {
                return Err(err("BATCH_INVALID"));
            }
            let weights = mixer::dirichlet_weights(ids.len(), k, seed);
            (ids, weights, dp)
        }
    };
    validate_params(&params)?;
    let mut files = TempFiles(Vec::new());
    let mut items = Vec::with_capacity(weights.len());
    for weights in weights {
        let (style, recipe) = build_mix(lab, ids.iter().copied().zip(weights).collect(), dp, None)?;
        let path = lab
            .root
            .join(format!("tmp/explore-{:x}.json", rand::random::<u64>()));
        files.0.push(path.clone());
        std::fs::write(&path, canonical_bytes(&style))?;
        items.push((path, recipe));
    }
    let (reference, encoder) = if let Some(region) = reference {
        if !models::status(&lab.root).encoder_installed {
            return Err(err("ENCODER_MISSING"));
        }
        let path = cut_reference(
            &lab.store,
            &lab.root,
            region.source_id,
            region.start_ms,
            region.end_ms,
            1000,
            60000,
        )?;
        files.0.push(path.clone());
        (Some(path), Some(models::encoder_path(&lab.root)))
    } else {
        (None, None)
    };
    let result = lab
        .runner
        .batch(
            &lab.store, &lab.root, items, params, reference, encoder, emit,
        )
        .await?;
    Ok(ExploreResult {
        items: result
            .items
            .into_iter()
            .map(|(render, similarity)| ExploreItem { render, similarity })
            .collect(),
        cancelled: result.cancelled,
    })
}

#[tauri::command]
pub async fn vl_explore(
    state: State<'_, VlState>,
    app: tauri::AppHandle,
    explore: Explore,
    params: RenderParams,
    reference: Option<RefRegion>,
) -> Result<ExploreResult, String> {
    let handle = tokio::runtime::Handle::current();
    blocking(lab(&state)?, move |l| {
        handle.block_on(self::explore(
            l,
            explore,
            params,
            reference,
            job_emitter(app),
        ))
    })
    .await
}

pub(super) fn check_export_path(path: &Path, ext: &str) -> Result<(), VlError> {
    if !path.is_absolute()
        || !path.parent().is_some_and(Path::is_dir)
        || path.extension().and_then(|s| s.to_str()) != Some(ext)
    {
        return Err(err("EXPORT_FAILED"));
    }
    Ok(())
}

fn export_bytes(path: &Path, ext: &str, bytes: &[u8]) -> Result<(), VlError> {
    check_export_path(path, ext)?;
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
    // Same limits as .svlab import, so every local profile stays exportable.
    if name.chars().count() > 200 || tags.len() > 20 || tags.iter().any(|t| t.chars().count() > 50)
    {
        return Err(err("NAME_INVALID"));
    }
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
    device: Option<CloneDevice>,
    emit: Emit,
) -> Result<u64, VlError> {
    let device = device.unwrap_or_default();
    if device == CloneDevice::Gpu {
        if !super::gpu::SUPPORTED {
            return Err(err("GPU_UNSUPPORTED"));
        }
        if super::gpu::pack(&lab.root) != super::gpu::Pack::Ok {
            return Err(err("GPU_PACK_MISSING"));
        }
    }
    let _gate = if device == CloneDevice::Gpu {
        Some(lab.gpu_gate.lock().map_err(|_| err("IO_FAILED"))?)
    } else {
        None
    };
    if device == CloneDevice::Gpu {
        let slot = lab.gpu_install.try_lock().map_err(|_| err("JOB_BUSY"))?;
        if slot.is_some() || lab.train.active().is_some() {
            return Err(err("JOB_BUSY"));
        }
    }
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
        device,
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
    device: Option<CloneDevice>,
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
            device,
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
            runner: Arc::new(Runner::new(
                exe.clone(),
                vec![("VL_FAKE_MODE".into(), "ok".into())],
            )),
            train: Arc::new(super::super::train_runner::TrainRunner::new(
                exe.with_file_name(format!("vl_fake_trainer{}", std::env::consts::EXE_SUFFIX)),
                "unused".into(),
                vec![("VL_FAKE_MODE".into(), "hang".into())],
                Duration::from_secs(1),
            )),
            train_install: tokio::sync::Mutex::new(None),
            gpu_install: tokio::sync::Mutex::new(None),
            gpu_gate: std::sync::Mutex::new(()),
            torch_cache: std::sync::Mutex::new(None),
            installing: tokio::sync::Mutex::new(()),
        };
        (d, lab, p)
    }

    #[test]
    fn gpu_clone_without_pack_fails() {
        let (_d, lab, _) = setup();
        let e = clone(
            &lab,
            1,
            0,
            1000,
            "words".into(),
            "en".into(),
            "GPU".into(),
            ClonePreset::Fast,
            42,
            Some(CloneDevice::Gpu),
            Arc::new(|_| {}),
        )
        .unwrap_err();
        assert_eq!(
            e.code(),
            if super::super::gpu::SUPPORTED {
                "GPU_PACK_MISSING"
            } else {
                "GPU_UNSUPPORTED"
            }
        );
        assert!(lab.runner.jobs().is_empty());
        assert!(!lab.runner.gpu_clone_running());
    }
    #[tokio::test]
    async fn train_start_blocked_during_gpu_clone() {
        let (_d, lab, _) = setup();
        let source = lab
            .store
            .lock()
            .unwrap()
            .add_source("test", "wav", "ref", 2000, true, &[])
            .unwrap();
        let path = lab.store.lock().unwrap().source_work_path(source.id);
        std::fs::write(
            path,
            audio::wav_bytes(&vec![0.1; audio::SR as usize * 2], audio::SR),
        )
        .unwrap();
        let id = lab
            .runner
            .start_clone(
                lab.store.clone(),
                lab.root.clone(),
                lab.root.join("models"),
                None,
                source.id,
                0,
                1000,
                "words".into(),
                "en".into(),
                "GPU".into(),
                ClonePreset::Fast,
                42,
                CloneDevice::Gpu,
                Arc::new(|_| {}),
            )
            .unwrap();
        assert_eq!(
            train_start(
                &lab,
                source.id,
                0,
                1000,
                "en".into(),
                "fast".into(),
                42,
                Arc::new(|_| {})
            )
            .unwrap_err()
            .code(),
            "JOB_BUSY"
        );
        assert_eq!(
            train_resume(&lab, 1, Arc::new(|_| {})).unwrap_err().code(),
            "JOB_BUSY"
        );
        lab.runner.cancel(id).unwrap();
        while lab.runner.gpu_clone_running() {
            tokio::task::yield_now().await;
        }
    }
    #[tokio::test]
    async fn vl_train_install_log_tails_file() {
        let (d, _, _) = setup();
        assert_eq!(train_install_log(d.path()).await.unwrap(), "");
        std::fs::create_dir_all(d.path().join("trainer")).unwrap();
        // Odd length puts the 8 KiB cut inside a two-byte character.
        let text = "я".repeat(5121) + "конец!";
        std::fs::write(d.path().join("trainer/install.log"), &text).unwrap();
        let tail = train_install_log(d.path()).await.unwrap();
        assert_eq!(tail.len(), 8191);
        assert!(text.ends_with(&tail));
        assert!(tail.ends_with("конец!"));
    }
    #[test]
    fn tts_synth_is_send() {
        fn assert_send<T: Send>(_: &T) {}
        let (_d, lab, p) = setup();
        assert_send(&tts_synth(
            Arc::new(lab),
            p.id,
            params(),
            PathBuf::from("tts.wav"),
        ));
    }
    #[tokio::test]
    async fn synth_file_writes_without_render() {
        let (_d, lab, p) = setup();
        let path = lab.store.lock().unwrap().profile_path(&p);
        let out = lab.root.join("tmp/tts.wav");
        assert_eq!(
            lab.runner
                .synth_file(&lab.root, path, params(), &out)
                .await
                .unwrap(),
            1000
        );
        assert_eq!(hound::WavReader::open(&out).unwrap().duration(), audio::SR);
        assert!(lab.store.lock().unwrap().renders(200).unwrap().is_empty());
        assert!(lab.runner.jobs().is_empty());
    }
    #[tokio::test]
    async fn synth_file_busy() {
        let (_d, lab, p) = setup();
        let mode = lab.root.join("mode");
        std::fs::write(&mode, "sleep").unwrap();
        let exe = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("examples")
            .join(format!("vl_fake_worker{}", std::env::consts::EXE_SUFFIX));
        let runner = Runner::new(
            exe,
            vec![(
                "VL_FAKE_MODE_FILE".into(),
                mode.to_string_lossy().into_owned(),
            )],
        );
        let path = lab.store.lock().unwrap().profile_path(&p);
        let out = lab.root.join("tmp/tts.wav");
        let first = runner.synth_file(&lab.root, path.clone(), params(), &out);
        tokio::pin!(first);
        tokio::select! {
            biased;
            result = &mut first => panic!("synth ended early: {result:?}"),
            _ = std::future::ready(()) => {}
        }
        assert_eq!(
            runner
                .synth_file(
                    &lab.root,
                    path.clone(),
                    params(),
                    &lab.root.join("tmp/other.wav")
                )
                .await
                .unwrap_err()
                .code(),
            "JOB_BUSY"
        );
        assert_eq!(
            runner
                .synth(
                    &lab.store,
                    &lab.root,
                    path,
                    Some(p.id),
                    None,
                    params(),
                    Arc::new(|_| {})
                )
                .await
                .unwrap_err()
                .code(),
            "JOB_BUSY"
        );
        assert!(runner.jobs().is_empty());
    }
    #[tokio::test]
    async fn synth_file_failure_leaves_no_file() {
        let (_d, lab, p) = setup();
        let exe = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("examples")
            .join(format!("vl_fake_worker{}", std::env::consts::EXE_SUFFIX));
        let runner = Runner::new(exe, vec![("VL_FAKE_MODE".into(), "crash".into())]);
        let path = lab.store.lock().unwrap().profile_path(&p);
        let out = lab.root.join("tmp/tts.wav");
        std::fs::write(&out, b"partial wav").unwrap();
        assert!(
            runner
                .synth_file(&lab.root, path, params(), &out)
                .await
                .is_err()
        );
        assert!(!out.exists());
        assert!(runner.jobs().is_empty());
    }

    fn explore_setup() -> (tempfile::TempDir, VoiceLab, i64, i64) {
        let (d, lab, a) = setup();
        for file in models::MODEL_FILES {
            let dest = models::model_dir(&lab.root).join(file.path);
            std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
            std::fs::File::create(dest)
                .unwrap()
                .set_len(file.size)
                .unwrap();
        }
        let b = lab
            .store
            .lock()
            .unwrap()
            .put_profile(
                &Style {
                    ttl: Array3::from_elem((1, 50, 256), 0.3),
                    dp: Array3::from_elem((1, 8, 16), 0.4),
                },
                "B",
                "import",
                None,
                None,
            )
            .unwrap();
        (d, lab, a.id, b.id)
    }

    #[tokio::test]
    async fn interp_returns_points_in_order() {
        let (_d, lab, a, b) = explore_setup();
        let result = explore(
            &lab,
            Explore::Interp { a, b, points: 5 },
            params(),
            None,
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
        assert!(!result.cancelled);
        assert_eq!(result.items.len(), 5);
        for (i, item) in result.items.iter().enumerate() {
            let recipe = item.render.recipe.as_ref().unwrap();
            assert_eq!(
                recipe.inputs.iter().map(|p| p.weight).collect::<Vec<_>>(),
                vec![100.0 - i as f64 * 25.0, i as f64 * 25.0]
            );
            assert_eq!(item.similarity, Some(i as f32 * 0.1));
            assert!(
                lab.store
                    .lock()
                    .unwrap()
                    .render_path(item.render.id)
                    .is_file()
            );
        }
        assert_eq!(lab.store.lock().unwrap().profiles().unwrap().len(), 2);
        assert_eq!(std::fs::read_dir(lab.root.join("tmp")).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn interp_uses_weighted_dp() {
        let (_d, lab, a, b) = explore_setup();
        let result = explore(
            &lab,
            Explore::Interp { a, b, points: 5 },
            params(),
            None,
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
        for item in result.items {
            assert_eq!(
                item.render.recipe.unwrap().dp_strategy,
                DpStrategy::Weighted
            );
        }
    }

    #[tokio::test]
    async fn random_same_seed_same_weights() {
        let (_d, lab, a, b) = explore_setup();
        let mut weights = Vec::new();
        for _ in 0..2 {
            let result = explore(
                &lab,
                Explore::Random {
                    ids: vec![a, b],
                    k: 4,
                    seed: 7,
                    dp: DpStrategy::Dominant,
                },
                params(),
                None,
                Arc::new(|_| {}),
            )
            .await
            .unwrap();
            assert_eq!(result.items.len(), 4);
            weights.push(
                result
                    .items
                    .into_iter()
                    .map(|item| {
                        item.render
                            .recipe
                            .unwrap()
                            .inputs
                            .into_iter()
                            .map(|p| p.weight)
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>(),
            );
        }
        assert_eq!(weights[0], weights[1]);
    }

    #[tokio::test]
    async fn random_select_rejected() {
        let (_d, lab, a, b) = explore_setup();
        let error = explore(
            &lab,
            Explore::Random {
                ids: vec![a, b],
                k: 4,
                seed: 7,
                dp: DpStrategy::Select,
            },
            params(),
            None,
            Arc::new(|_| {}),
        )
        .await
        .err()
        .unwrap();
        assert_eq!(error.code(), "BATCH_INVALID");
        assert!(lab.runner.jobs().is_empty());
    }

    #[tokio::test]
    async fn explore_text_over_300_rejected() {
        let (_d, lab, a, b) = explore_setup();
        for text in ["я".repeat(301), "я".repeat(1001), " ".into()] {
            let mut p = params();
            p.text = text;
            let error = explore(
                &lab,
                Explore::Interp { a, b, points: 3 },
                p,
                None,
                Arc::new(|_| {}),
            )
            .await
            .err()
            .unwrap();
            assert_eq!(error.code(), "BATCH_INVALID");
        }
        let mut p = params();
        p.text = format!(" {} ", "я".repeat(300));
        assert_eq!(
            explore(
                &lab,
                Explore::Interp { a, b, points: 3 },
                p,
                None,
                Arc::new(|_| {})
            )
            .await
            .unwrap()
            .items
            .len(),
            3
        );
    }

    #[tokio::test]
    async fn explore_reference_without_encoder() {
        let (_d, lab, a, b) = explore_setup();
        let error = explore(
            &lab,
            Explore::Interp { a, b, points: 3 },
            params(),
            Some(RefRegion {
                source_id: 1,
                start_ms: 0,
                end_ms: 1000,
            }),
            Arc::new(|_| {}),
        )
        .await
        .err()
        .unwrap();
        assert_eq!(error.code(), "ENCODER_MISSING");
        assert!(lab.runner.jobs().is_empty());
        assert_eq!(std::fs::read_dir(lab.root.join("tmp")).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn explore_cancel_returns_partial() {
        let (_d, mut lab, a, b) = explore_setup();
        let exe = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("examples")
            .join(format!("vl_fake_worker{}", std::env::consts::EXE_SUFFIX));
        lab.runner = Arc::new(Runner::new(
            exe,
            vec![("VL_FAKE_MODE".into(), "batch_slow".into())],
        ));
        let runner = lab.runner.clone();
        let emit: Emit = Arc::new(move |job| {
            if matches!(
                job.progress,
                Some(super::super::worker::WorkerEvent::Item { index: 1, .. })
            ) && job.status == "running"
            {
                runner.cancel(job.id).unwrap();
            }
        });
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            explore(
                &lab,
                Explore::Interp { a, b, points: 5 },
                params(),
                None,
                emit,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(result.cancelled);
        assert_eq!(result.items.len(), 2);
        assert_eq!(lab.store.lock().unwrap().renders(200).unwrap().len(), 2);
        assert_eq!(std::fs::read_dir(lab.root.join("tmp")).unwrap().count(), 0);
        assert_eq!(
            explore(
                &lab,
                Explore::Interp { a, b, points: 3 },
                params(),
                None,
                Arc::new(|_| {})
            )
            .await
            .unwrap()
            .items
            .len(),
            3
        );
    }

    #[test]
    fn start_requires_pack() {
        let (d, lab, _) = setup();
        assert_eq!(
            train_start(
                &lab,
                1,
                0,
                3000,
                "ru".into(),
                "fast".into(),
                42,
                Arc::new(|_| {})
            )
            .unwrap_err()
            .code(),
            "TRAINER_NOT_INSTALLED"
        );
        std::fs::create_dir_all(d.path().join("trainer")).unwrap();
        std::fs::write(d.path().join("trainer/installed.ok"), "old").unwrap();
        assert_eq!(
            train_start(
                &lab,
                1,
                0,
                3000,
                "ru".into(),
                "fast".into(),
                42,
                Arc::new(|_| {})
            )
            .unwrap_err()
            .code(),
            "TRAINER_OUTDATED"
        );
    }
    #[test]
    fn resume_requires_pack() {
        let (d, lab, _) = setup();
        assert_eq!(
            train_resume(&lab, 1, Arc::new(|_| {})).unwrap_err().code(),
            "TRAINER_NOT_INSTALLED"
        );
        std::fs::create_dir_all(d.path().join("trainer")).unwrap();
        std::fs::write(d.path().join("trainer/installed.ok"), "old").unwrap();
        assert_eq!(
            train_resume(&lab, 1, Arc::new(|_| {})).unwrap_err().code(),
            "TRAINER_OUTDATED"
        );
    }
    async fn active_run_checks() {
        let (_d, lab, _) = setup();
        let source = lab
            .store
            .lock()
            .unwrap()
            .add_source("a", "wav", "reference", 3000, true, &[])
            .unwrap();
        std::fs::write(
            lab.store.lock().unwrap().source_work_path(source.id),
            audio::wav_bytes(&vec![0.1; audio::SR as usize * 3], audio::SR),
        )
        .unwrap();
        let id = lab
            .train
            .start(
                lab.store.clone(),
                lab.root.clone(),
                source.id,
                0,
                3000,
                "ru".into(),
                train_params("fast", 42).unwrap(),
                Arc::new(|_| {}),
            )
            .unwrap();
        assert_eq!(train_delete(&lab, id).unwrap_err().code(), "JOB_BUSY");
        assert_eq!(train_uninstall(&lab).unwrap_err().code(), "JOB_BUSY");
        lab.train.kill(id);
        while lab.train.active().is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    #[tokio::test]
    async fn delete_active_run_busy() {
        active_run_checks().await;
    }
    #[tokio::test]
    async fn uninstall_during_run_busy() {
        active_run_checks().await;
    }
    #[tokio::test]
    async fn status_during_install_is_ok() {
        let (_d, lab, _) = setup();
        let (tx, _rx) = tokio::sync::oneshot::channel();
        *lab.train_install.lock().await = Some(tx);
        let status = train_status(&lab, false).await.unwrap();
        let json = serde_json::to_value(status).unwrap();
        assert_eq!(json["installing"], true);
        assert!(json.get("pack").is_some());
        assert!(lab.train_install.try_lock().is_ok());
    }

    #[tokio::test]
    async fn install_cleanup_with_poisoned_cache() {
        let (_d, lab, _) = setup();
        let (tx, _rx) = tokio::sync::oneshot::channel();
        *lab.train_install.lock().await = Some(tx);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = lab.torch_cache.lock().unwrap();
            panic!("poison cache");
        }));
        finish_train_install(&lab).await;
        assert!(lab.train_install.lock().await.is_none());
        assert!(lab.torch_cache.lock().unwrap_err().into_inner().is_none());
    }

    #[test]
    fn train_presets_and_model_required() {
        let (d, lab, _) = setup();
        for (preset, total) in [("fast", 1000), ("standard", 3000), ("long", 6000)] {
            let p = train_params(preset, 123).unwrap();
            assert_eq!((p.total_steps, p.save_every, p.seed), (total, 500, 123));
            assert_eq!((p.lr, p.threshold), (2e-4, 0.24));
        }
        assert_eq!(
            train_params("unknown", 42).unwrap_err().code(),
            "TRAIN_INVALID"
        );
        let dir = d.path().join("trainer/weights/wavlm-large");
        std::fs::create_dir_all(&dir).unwrap();
        for file in trainer::WAVLM_FILES {
            std::fs::File::create(dir.join(file.path))
                .unwrap()
                .set_len(file.size)
                .unwrap();
        }
        std::fs::write(d.path().join("trainer/installed.ok"), trainer::lock_hash()).unwrap();
        assert_eq!(
            train_start(
                &lab,
                1,
                0,
                3000,
                "ru".into(),
                "fast".into(),
                42,
                Arc::new(|_| {})
            )
            .unwrap_err()
            .code(),
            "MODEL_MISSING"
        );
        assert_eq!(
            train_resume(&lab, 1, Arc::new(|_| {})).unwrap_err().code(),
            "MODEL_MISSING"
        );
    }
    #[test]
    fn install_blocks_start_resume_and_uninstall() {
        let (_d, lab, _) = setup();
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        *lab.train_install.try_lock().unwrap() = Some(tx);
        assert_eq!(
            train_start(
                &lab,
                1,
                0,
                3000,
                "ru".into(),
                "fast".into(),
                42,
                Arc::new(|_| {})
            )
            .unwrap_err()
            .code(),
            "JOB_BUSY"
        );
        assert_eq!(
            train_resume(&lab, 1, Arc::new(|_| {})).unwrap_err().code(),
            "JOB_BUSY"
        );
        assert_eq!(train_uninstall(&lab).unwrap_err().code(), "JOB_BUSY");
        assert_eq!(
            rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        );
    }
    #[test]
    fn uninstall_resets_torch_cache() {
        let (d, lab, _) = setup();
        std::fs::create_dir_all(d.path().join("trainer")).unwrap();
        *lab.torch_cache.lock().unwrap() = Some(Some("12.8".into()));
        train_uninstall(&lab).unwrap();
        assert_eq!(*lab.torch_cache.lock().unwrap(), None);
        assert!(!d.path().join("trainer").exists());
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

fn require_train(lab: &VoiceLab) -> Result<(), VlError> {
    match trainer::pack_state(&lab.root) {
        "ok" => require_model(&lab.root),
        "outdated" => Err(err("TRAINER_OUTDATED")),
        _ => Err(err("TRAINER_NOT_INSTALLED")),
    }
}
fn train_params(preset: &str, seed: u64) -> Result<TrainParams, VlError> {
    let total_steps = match preset {
        "fast" => 1000,
        "standard" => 3000,
        "long" => 6000,
        _ => return Err(err("TRAIN_INVALID")),
    };
    Ok(TrainParams {
        preset: preset.into(),
        total_steps,
        lr: 2e-4,
        save_every: 500,
        seed,
        threshold: 0.24,
    })
}
type TrainEmit = Arc<dyn Fn(&TrainRun) + Send + Sync>;
fn train_emitter(app: tauri::AppHandle) -> TrainEmit {
    Arc::new(move |run| {
        let _ = app.emit("vl-train", run);
    })
}
#[allow(clippy::too_many_arguments)]
fn train_start(
    lab: &VoiceLab,
    source_id: i64,
    start_ms: u64,
    end_ms: u64,
    lang: String,
    preset: String,
    seed: u64,
    emit: TrainEmit,
) -> Result<i64, VlError> {
    let _gate = lab.gpu_gate.lock().map_err(|_| err("IO_FAILED"))?;
    if lab.runner.gpu_clone_running() {
        return Err(err("JOB_BUSY"));
    }
    let slot = lab.train_install.try_lock().map_err(|_| err("JOB_BUSY"))?;
    if slot.is_some() {
        return Err(err("JOB_BUSY"));
    }
    require_train(lab)?;
    if !AVAILABLE_LANGS.contains(&lang.as_str()) {
        return Err(err("TRAIN_INVALID"));
    }
    lab.train.start(
        lab.store.clone(),
        lab.root.clone(),
        source_id,
        start_ms,
        end_ms,
        lang,
        train_params(&preset, seed)?,
        emit,
    )
}
fn train_resume(lab: &VoiceLab, run_id: i64, emit: TrainEmit) -> Result<(), VlError> {
    let _gate = lab.gpu_gate.lock().map_err(|_| err("IO_FAILED"))?;
    if lab.runner.gpu_clone_running() {
        return Err(err("JOB_BUSY"));
    }
    let slot = lab.train_install.try_lock().map_err(|_| err("JOB_BUSY"))?;
    if slot.is_some() {
        return Err(err("JOB_BUSY"));
    }
    require_train(lab)?;
    lab.train
        .resume(lab.store.clone(), lab.root.clone(), run_id, emit)
}
fn train_delete(lab: &VoiceLab, run_id: i64) -> Result<(), VlError> {
    lab.train.delete_run(&lab.store, run_id)
}
fn train_uninstall(lab: &VoiceLab) -> Result<(), VlError> {
    let slot = lab.train_install.try_lock().map_err(|_| err("JOB_BUSY"))?;
    if slot.is_some() || lab.train.active().is_some() {
        return Err(err("JOB_BUSY"));
    }
    *lab.torch_cache.lock().map_err(|_| err("IO_FAILED"))? = None;
    trainer::uninstall(&lab.root)
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrainStatus {
    #[serde(flatten)]
    diagnostics: trainer::TrainStatus,
    installing: bool,
    languages: &'static [&'static str],
}

async fn train_status(l: &VoiceLab, refresh: bool) -> Result<TrainStatus, String> {
    let installing = l.train_install.lock().await.is_some();
    Ok(TrainStatus {
        diagnostics: trainer::status(&l.root, &l.torch_cache, refresh).await,
        installing,
        languages: AVAILABLE_LANGS,
    })
}

#[tauri::command]
pub async fn vl_train_status(
    state: State<'_, VlState>,
    refresh: bool,
) -> Result<TrainStatus, String> {
    let l = lab(&state)?;
    train_status(&l, refresh).await
}

async fn finish_train_install(l: &VoiceLab) {
    let mut slot = l.train_install.lock().await;
    *l.torch_cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    slot.take();
}

async fn train_install_log(root: &Path) -> Result<String, String> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let mut file = match tokio::fs::File::open(trainer::trainer_dir(root).join("install.log")).await
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(_) => return Err("TRAINER_INSTALL_FAILED".into()),
    };
    let len = file
        .metadata()
        .await
        .map_err(|_| "TRAINER_INSTALL_FAILED")?
        .len();
    file.seek(std::io::SeekFrom::Start(len.saturating_sub(8192)))
        .await
        .map_err(|_| "TRAINER_INSTALL_FAILED")?;
    let mut bytes = Vec::new();
    file.take(8192)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| "TRAINER_INSTALL_FAILED")?;
    let start = bytes
        .iter()
        .position(|b| b & 0xc0 != 0x80)
        .unwrap_or(bytes.len());
    Ok(String::from_utf8_lossy(&bytes[start..]).into_owned())
}

#[tauri::command]
pub async fn vl_train_install_log(state: State<'_, VlState>) -> Result<String, String> {
    train_install_log(&lab(&state)?.root).await
}

#[tauri::command]
pub async fn vl_train_install(
    state: State<'_, VlState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let l = lab(&state)?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    {
        let mut slot = l
            .train_install
            .try_lock()
            .map_err(|_| "JOB_BUSY".to_string())?;
        if slot.is_some() || l.train.active().is_some() {
            return Err("JOB_BUSY".into());
        }
        *slot = Some(tx);
        *l.torch_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
    let last = std::sync::Mutex::new(None::<Instant>);
    let progress = |stage: &str, done, total, item: Option<&str>| {
        let mut last = last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if done == total || last.is_none_or(|t| t.elapsed() >= Duration::from_millis(200)) {
            let _ = app.emit(
                "vl-train-install",
                serde_json::json!({"stage":stage,"done":done,"total":total,"item":item}),
            );
            *last = Some(Instant::now());
        }
    };
    let result = trainer::install(&l.root, &progress, rx).await;
    finish_train_install(&l).await;
    result.map_err(|e| e.code())
}
#[tauri::command]
pub async fn vl_train_install_cancel(state: State<'_, VlState>) -> Result<(), String> {
    // Keep the slot occupied until install has finished cleaning up.
    let l = lab(&state)?;
    let mut slot = l.train_install.lock().await;
    if slot.is_some() {
        let (placeholder, _rx) = tokio::sync::oneshot::channel();
        if let Some(tx) = slot.replace(placeholder) {
            let _ = tx.send(());
        }
    }
    Ok(())
}
#[tauri::command]
pub async fn vl_train_uninstall(state: State<'_, VlState>) -> Result<(), String> {
    blocking(lab(&state)?, train_uninstall).await
}
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn vl_train_start(
    state: State<'_, VlState>,
    app: tauri::AppHandle,
    source_id: i64,
    start_ms: u64,
    end_ms: u64,
    lang: String,
    preset: String,
    seed: u64,
) -> Result<i64, String> {
    blocking(lab(&state)?, move |l| {
        train_start(
            l,
            source_id,
            start_ms,
            end_ms,
            lang,
            preset,
            seed,
            train_emitter(app),
        )
    })
    .await
}
#[tauri::command]
pub async fn vl_train_resume(
    state: State<'_, VlState>,
    app: tauri::AppHandle,
    run_id: i64,
) -> Result<(), String> {
    blocking(lab(&state)?, move |l| {
        train_resume(l, run_id, train_emitter(app))
    })
    .await
}
#[tauri::command]
pub async fn vl_train_stop(state: State<'_, VlState>, run_id: i64) -> Result<(), String> {
    lab(&state)?.train.stop(run_id).map_err(|e| e.code())
}
#[tauri::command]
pub async fn vl_train_kill(state: State<'_, VlState>, run_id: i64) -> Result<(), String> {
    lab(&state)?.train.kill(run_id);
    Ok(())
}
#[tauri::command]
pub async fn vl_train_runs(state: State<'_, VlState>) -> Result<Vec<TrainRun>, String> {
    blocking(lab(&state)?, |l| {
        l.store.lock().map_err(|_| err("IO_FAILED"))?.runs()
    })
    .await
}
#[tauri::command]
pub async fn vl_train_delete(state: State<'_, VlState>, run_id: i64) -> Result<(), String> {
    blocking(lab(&state)?, move |l| train_delete(l, run_id)).await
}

#[tauri::command]
pub async fn vl_export_project(state: State<'_, VlState>, path: PathBuf) -> Result<(), String> {
    blocking(lab(&state)?, move |l| {
        super::project::export_project(&l.store, &path)
    })
    .await
}

#[tauri::command]
pub async fn vl_import_project(
    state: State<'_, VlState>,
    path: PathBuf,
) -> Result<super::project::ImportReport, String> {
    blocking(lab(&state)?, move |l| {
        super::project::import_project(&l.store, &path)
    })
    .await
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuStatus {
    pub supported: bool,
    pub pack: super::gpu::Pack,
    pub installing: bool,
    pub gpu: Option<trainer::GpuInfo>,
    pub missing_libs: Vec<String>,
}

#[tauri::command]
pub async fn vl_gpu_status(state: State<'_, VlState>) -> Result<GpuStatus, String> {
    let l = lab(&state)?;
    let installing = l.gpu_install.lock().await.is_some();
    let gpu = trainer::gpu_info().await;
    blocking(l, move |l| {
        let pack = super::gpu::pack(&l.root);
        let missing_libs = if pack == super::gpu::Pack::Ok {
            super::gpu::missing_libs(&l.root)
        } else {
            Vec::new()
        };
        Ok(GpuStatus {
            supported: super::gpu::SUPPORTED,
            pack,
            installing,
            gpu,
            missing_libs,
        })
    })
    .await
}

#[tauri::command]
pub async fn vl_gpu_install(
    state: State<'_, VlState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    if !super::gpu::SUPPORTED {
        return Err("GPU_UNSUPPORTED".into());
    }
    let l = lab(&state)?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    {
        let _gate = l.gpu_gate.lock().map_err(|_| "IO_FAILED".to_string())?;
        if l.runner.gpu_clone_running() {
            return Err("JOB_BUSY".into());
        }
        let mut slot = l
            .gpu_install
            .try_lock()
            .map_err(|_| "JOB_BUSY".to_string())?;
        if slot.is_some() {
            return Err("JOB_BUSY".into());
        }
        *slot = Some(tx);
    }
    let last = std::sync::Mutex::new(None::<Instant>);
    let progress = |done, total| {
        let mut last = last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if done == total || last.is_none_or(|t| t.elapsed() >= Duration::from_millis(200)) {
            let _ = app.emit(
                "vl-gpu-install",
                serde_json::json!({"done": done, "total": total}),
            );
            *last = Some(Instant::now());
        }
    };
    let result = super::gpu::install(&l.root, &progress, rx).await;
    *l.gpu_install.lock().await = None;
    result.map_err(|e| e.code())
}

#[tauri::command]
pub async fn vl_gpu_install_cancel(state: State<'_, VlState>) -> Result<(), String> {
    let l = lab(&state)?;
    let mut slot = l.gpu_install.lock().await;
    if slot.is_some() {
        let (placeholder, _rx) = tokio::sync::oneshot::channel();
        if let Some(tx) = slot.replace(placeholder) {
            let _ = tx.send(());
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn vl_gpu_uninstall(state: State<'_, VlState>) -> Result<(), String> {
    blocking(lab(&state)?, |l| {
        let _gate = l.gpu_gate.lock().map_err(|_| err("IO_FAILED"))?;
        if l.runner.gpu_clone_running() {
            return Err(err("JOB_BUSY"));
        }
        let slot = l.gpu_install.try_lock().map_err(|_| err("JOB_BUSY"))?;
        if slot.is_some() {
            return Err(err("JOB_BUSY"));
        }
        super::gpu::uninstall(&l.root)
    })
    .await
}
