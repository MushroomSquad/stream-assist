import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
export interface ModelStatus { installed: boolean; encoderInstalled: boolean; ffmpeg: boolean }
export interface Source { id: number; name: string; durationMs: number; consent: boolean; createdAt: number }
export type DpStrategy = 'dominant' | 'weighted' | 'select';
export type ClonePreset = 'fast' | 'standard' | 'precise';
export interface Recipe { schemaVersion: number; kind: string; engine: { id: string; version: string; assetRevision: string }; inputs: { profileSha256: string; name: string; weight: number }[]; dpStrategy: DpStrategy; selectedDpProfileSha256: string | null; createdAt: string }
export interface Profile { id: number; name: string; tags: string[]; styleSha256: string; origin: 'builtin' | 'import' | 'clone' | 'mix'; recipe: Recipe | null; meta: unknown; createdAt: number }
export interface RenderParams { text: string; lang: string; speed: number; quality: number }
export interface Render extends RenderParams { id: number; profileId: number | null; recipe: Recipe | null; durationMs: number; wallMs: number; favorite: boolean; createdAt: number }
export interface JobState { id: number; kind: string; status: string; progress: { event: string; stage?: string; iteration?: number; total?: number; loss?: number; bestLoss?: number } | null; error: string | null; profileId: number | null; renderId: number | null }
function call<T>(name: string, args?: Record<string, unknown>): Promise<T> { return isTauri() ? invoke<T>(name, args) : Promise.reject(new Error('browser')); }
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
export const onJob = (cb: (job: JobState) => void) => listen<JobState>('vl-job', e => cb(e.payload));
export const onInstall = (cb: (p: { done: number; total: number }) => void) => listen<{ done: number; total: number }>('vl-install', e => cb(e.payload));
export const blobUrl = (buf: ArrayBuffer) => URL.createObjectURL(new Blob([buf], { type: 'audio/wav' }));
