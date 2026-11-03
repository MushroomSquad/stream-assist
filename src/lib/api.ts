import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { openUrl } from '@tauri-apps/plugin-opener';
import { open } from '@tauri-apps/plugin-dialog';
import type { TtsSettings } from './ttsApi';
import { ru } from './i18n/ru';
export interface Track { title: string; artist: string; duration_ms: number | null }
export interface QueueItem { id: number; track: Track; requester: string }
export interface WidgetState { visible: boolean; now: QueueItem | null; queue: QueueItem[]; progress_ms: number; mode: string; warning: string | null }
export interface Settings { tts: TtsSettings; limits: { max_duration_ms: number | null; no_repeat_last_n: number; max_per_user_in_queue: number | null; block_explicit: boolean; blocklist: string[] }; reward_title: string; reward_cost: number; reward_cooldown_s: number; port: number; active_theme: string; youtube_api_key: string | null; obs: { host: string; port: number; password: string | null }; music_reward_id: string | null; spotify_client_id: string | null }
export interface ThemeMeta { id: string; name: string; author: string; width: number; height: number; parts?: { now: { width: number; height: number }; queue: { width: number; height: number } } }
export interface DeviceStart { user_code: string; verification_uri: string; interval: number; expires_in: number }
export interface SpotifyStatus { connected: boolean; premium: boolean | null }
export const emptyState: WidgetState = { visible: false, now: null, queue: [], progress_ms: 0, mode: 'Idle', warning: null };
export const defaultSettings: Settings = { tts: { reward_enabled: false, chat_enabled: false, profile_id: null, lang: 'ru', speed: 1, reward_title: ru.ttsRewardDefault, reward_cost: 300, reward_cooldown_s: 0, reward_id: null, marker: '%', roles: { everyone: true, subscribers: true, vips: true, moderators: true }, global_cooldown_s: 10, user_cooldown_s: 60, strip_links: true, stop_words: [] }, limits: { max_duration_ms: 420000, no_repeat_last_n: 10, max_per_user_in_queue: 2, block_explicit: false, blocklist: [] }, reward_title: ru.rewardDefault, reward_cost: 500, reward_cooldown_s: 0, port: 47821, active_theme: 'underground', youtube_api_key: null, obs: { host: 'localhost', port: 4455, password: null }, music_reward_id: null, spotify_client_id: null };
function call<T>(name: string, args?: Record<string, unknown>): Promise<T> { return isTauri() ? invoke<T>(name, args) : Promise.reject(new Error('browser')); }
export const get_state = () => isTauri() ? call<WidgetState>('get_state') : Promise.resolve(emptyState);
export const get_settings = () => isTauri() ? call<Settings>('get_settings') : Promise.resolve(structuredClone(defaultSettings));
export const skip = () => call<void>('skip');
export const remove_item = (id: number) => call<void>('remove_item', { id });
export const move_item = (id: number, to: number) => call<void>('move_item', { id, to });
export const toggle_pause = () => call<void>('toggle_pause');
export const resume_queue = () => call<void>('resume_queue');
export const set_volume = (pct: number) => call<void>('set_volume', { pct });
export const save_settings = (s: Settings) => call<void>('save_settings', { s });
export const twitch_login_start = () => call<DeviceStart>('twitch_login_start');
export const twitch_login_poll = () => call<boolean>('twitch_login_poll');
export const twitch_logout = () => call<void>('twitch_logout');
export const spotify_login = (client_id: string) => call<string>('spotify_login', { clientId: client_id });
export const spotify_status = () => call<SpotifyStatus>('spotify_status');
export const obs_check = () => call<string>('obs_check');
export const obs_add_widget = (scale = 1) => call<void>('obs_add_widget', { scale });
export const list_themes = () => isTauri() ? call<ThemeMeta[]>('list_themes') : Promise.resolve<ThemeMeta[]>(['underground', 'glass', 'crt', 'block'].map(id => ({ id, name: id, author: '', width: 800, height: 200 })));
export const import_theme = (path: string) => call<string>('import_theme', { path });
export const widget_test = () => call<void>('widget_test');
export const onState = (callback: (s: WidgetState) => void) => isTauri() ? listen<WidgetState>('state', e => callback(e.payload)) : Promise.resolve(() => {});
export const openExternal = (url: string) => isTauri() ? openUrl(url) : Promise.reject(new Error('browser'));
export const chooseTheme = () => isTauri() ? open({ multiple: false, directory: false, filters: [{ name: ru.zip, extensions: ['zip'] }] }) : Promise.reject(new Error('browser'));

export const running_port = () => isTauri() ? call<number | null>("running_port") : Promise.resolve(defaultSettings.port);
export const twitch_status = () => call<boolean>("twitch_status");

export const spotify_volume = () => isTauri() ? call<number | null>("spotify_volume") : Promise.resolve(null);
