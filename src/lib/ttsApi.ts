import { mockEnabled, mockCall } from './mock';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
export interface TtsCfg {
  profile_id: number | null; lang: string; speed: number;
  roles: { everyone: boolean; subscribers: boolean; vips: boolean; moderators: boolean };
  global_cooldown_s: number; user_cooldown_s: number; strip_links: boolean; stop_words: string[];
}
export interface ItemView { id: number; user: string; text: string; reward: boolean; synthesizing: boolean }
export interface TtsView { current: ItemView | null; queue: ItemView[]; widget: boolean; warning: string | null }
function call<T>(name: string, args?: Record<string, unknown>): Promise<T> { return isTauri() ? (args ? invoke<T>(name, args) : invoke<T>(name)) : mockEnabled ? mockCall<T>(name, args) : Promise.reject(new Error('browser')); }
export const tts_state = () => call<TtsView>('tts_state');
export const tts_skip = () => call<void>('tts_skip');
export const tts_clear = () => call<void>('tts_clear');
export const tts_test = (text: string) => call<void>('tts_test', { text });
export const obs_add_tts = () => call<void>('obs_add_tts');
export const onTts = (callback: (view: TtsView) => void) => isTauri() ? listen<TtsView>('tts', e => callback(e.payload)) : Promise.resolve(() => {});
