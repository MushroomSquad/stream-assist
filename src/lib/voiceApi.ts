import { mockEnabled, mockCall } from './mock';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
export interface ModelStatus { installed: boolean; encoderInstalled: boolean; ffmpeg: boolean }
export interface Source { id: number; name: string; durationMs: number; consent: boolean; createdAt: number }
export type DpStrategy = 'dominant' | 'weighted' | 'select';
export type ClonePreset = 'fast' | 'standard' | 'precise';
export interface Recipe { schemaVersion: number; kind: string; engine: { id: string; version: string; assetRevision: string }; inputs: { profileSha256: string; name: string; weight: number }[]; dpStrategy: DpStrategy; selectedDpProfileSha256: string | null; createdAt: string }
export interface Profile { id: number; name: string; tags: string[]; styleSha256: string; origin: 'builtin' | 'import' | 'clone' | 'mix' | 'train'; recipe: Recipe | null; meta: unknown; createdAt: number }
export interface RenderParams { text: string; lang: string; speed: number; quality: number }
export interface Render extends RenderParams { id: number; profileId: number | null; recipe: Recipe | null; durationMs: number; wallMs: number; favorite: boolean; createdAt: number }
export interface JobState { id: number; kind: string; status: string; progress: { event: string; index?: number; similarity?: number | null; stage?: string; iteration?: number; total?: number; loss?: number; bestLoss?: number } | null; error: string | null; profileId: number | null; renderId: number | null }
function call<T>(name: string, args?: Record<string, unknown>): Promise<T> { return isTauri() ? invoke<T>(name, args) : mockEnabled ? mockCall<T>(name, args) : Promise.reject(new Error('browser')); }
export const vl_status = () => call<ModelStatus>('vl_status');
export const vl_install = (withEncoder: boolean) => call<void>('vl_install', { withEncoder });
export const vl_sources = () => call<Source[]>('vl_sources');
export const vl_import_audio = (path: string, consent: boolean) => call<Source>('vl_import_audio', { path, consent });
export const vl_set_consent = (id: number, consent: boolean) => call<void>('vl_set_consent', { id, consent });
export const vl_peaks = (sourceId: number) => call<number[]>('vl_peaks', { sourceId });
export const vl_source_audio = (sourceId: number, startMs: number, endMs: number) => call<ArrayBuffer>('vl_source_audio', { sourceId, startMs, endMs });
export const vl_clone = (sourceId: number, startMs: number, endMs: number, transcript: string, lang: string, name: string, preset: ClonePreset, seed: number) => call<number>('vl_clone', { sourceId, startMs, endMs, transcript, lang, name, preset, seed });
export const vl_cancel_job = (id: number) => call<void>('vl_cancel_job', { id });
export const vl_jobs = () => call<JobState[]>('vl_jobs');
export const vl_profiles = () => call<Profile[]>('vl_profiles');
export const vl_import_profile = (path: string, name: string) => call<Profile>('vl_import_profile', { path, name });
export const vl_rename_profile = (id: number, name: string, tags: string[]) => call<void>('vl_rename_profile', { id, name, tags });
export const vl_delete_profile = (id: number) => call<void>('vl_delete_profile', { id });
export const vl_render = (profileId: number, params: RenderParams) => call<Render>('vl_render', { profileId, params });
export const vl_mix_preview = (inputs: [number, number][], dp: DpStrategy, selectId: number | null, params: RenderParams) => call<Render>('vl_mix_preview', { inputs, dp, selectId, params });
export const vl_save_snapshot = (inputs: [number, number][], dp: DpStrategy, selectId: number | null, name: string) => call<Profile>('vl_save_snapshot', { inputs, dp, selectId, name });
export const vl_renders = () => call<Render[]>('vl_renders');
export const vl_render_audio = (id: number) => call<ArrayBuffer>('vl_render_audio', { id });
export const vl_set_favorite = (id: number, fav: boolean) => call<void>('vl_set_favorite', { id, fav });
export const vl_export_profile = (id: number, path: string) => call<void>('vl_export_profile', { id, path });
export const vl_export_recipe = (profileId: number, path: string) => call<void>('vl_export_recipe', { profileId, path });
export const vl_export_render = (id: number, path: string) => call<void>('vl_export_render', { id, path });
export const onJob = (cb: (job: JobState) => void) => isTauri() ? listen<JobState>('vl-job', e => cb(e.payload)) : Promise.resolve(() => {});
export const onInstall = (cb: (p: { done: number; total: number }) => void) => isTauri() ? listen<{ done: number; total: number }>('vl-install', e => cb(e.payload)) : Promise.resolve(() => {});
export const blobUrl = (buf: ArrayBuffer) => URL.createObjectURL(new Blob([buf], { type: 'audio/wav' }));

export type TrainPreset = 'fast' | 'standard' | 'long';
export interface TrainParams { preset: TrainPreset; totalSteps: number; lr: number; saveEvery: number; seed: number; threshold: number }
export interface TrainRun { id: number; sourceId: number | null; startMs: number; endMs: number; lang: string; params: TrainParams; status: string; step: number; total: number; bestLoss: number | null; history: [number, number, number][]; error: string | null; lr: number | null; createdAt: number; updatedAt: number }
export interface TrainStatus { installing: boolean; languages: string[]; uv: string | null; uvOk: boolean; pack: 'none' | 'ok' | 'outdated'; gpu: { name: string; driver: string; vramTotalMb: number; vramFreeMb: number } | null; torchCuda: string | null; diskFreeMb: number | null }
export interface TrainInstallProgress { stage: string; done: number; total: number; item?: string | null }
export const vl_train_status = (refresh = false) => call<TrainStatus>('vl_train_status', { refresh });
export const vl_train_install = () => call<void>('vl_train_install');
export const vl_train_install_log = () => call<string>('vl_train_install_log');
export const vl_train_install_cancel = () => call<void>('vl_train_install_cancel');
export const vl_train_uninstall = () => call<void>('vl_train_uninstall');
export const vl_train_start = (sourceId: number, startMs: number, endMs: number, lang: string, preset: TrainPreset, seed: number) => call<number>('vl_train_start', { sourceId, startMs, endMs, lang, preset, seed });
export const vl_train_resume = (runId: number) => call<void>('vl_train_resume', { runId });
export const vl_train_stop = (runId: number) => call<void>('vl_train_stop', { runId });
export const vl_train_kill = (runId: number) => call<void>('vl_train_kill', { runId });
export const vl_train_runs = () => call<TrainRun[]>('vl_train_runs');
export const vl_train_delete = (runId: number) => call<void>('vl_train_delete', { runId });
export const onTrain = (cb: (run: TrainRun) => void) => isTauri() ? listen<TrainRun>('vl-train', e => cb(e.payload)) : Promise.resolve(() => {});
export const onTrainInstall = (cb: (progress: TrainInstallProgress) => void) => isTauri() ? listen<TrainInstallProgress>('vl-train-install', e => cb(e.payload)) : Promise.resolve(() => {});

export type Explore = { mode: 'interp'; a: number; b: number; points: number } | { mode: 'random'; ids: number[]; k: number; seed: number; dp: Exclude<DpStrategy, 'select'> };
export interface RefRegion { sourceId: number; startMs: number; endMs: number }
export interface ExploreItem { render: Render; similarity: number | null }
export interface ExploreResult { items: ExploreItem[]; cancelled: boolean }
export interface ImportReport { profiles: number; renders: number; skipped: number; revisionMismatch: boolean }
export const vl_explore = (explore: Explore, params: RenderParams, reference: RefRegion | null) => call<ExploreResult>('vl_explore', { explore, params, reference });
export const vl_export_project = (path: string) => call<void>('vl_export_project', { path });
export const vl_import_project = (path: string) => call<ImportReport>('vl_import_project', { path });
