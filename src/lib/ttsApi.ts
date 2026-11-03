import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
export interface TtsSettings {
  reward_enabled: boolean; chat_enabled: boolean; profile_id: number | null;
  lang: string; speed: number; reward_title: string; reward_cost: number;
  reward_cooldown_s: number; reward_id: string | null; marker: string;
  roles: { everyone: boolean; subscribers: boolean; vips: boolean; moderators: boolean };
  global_cooldown_s: number; user_cooldown_s: number; strip_links: boolean; stop_words: string[];
}
export interface ItemView { id: number; user: string; text: string; reward: boolean; synthesizing: boolean }
export interface TtsView { current: ItemView | null; queue: ItemView[]; widget: boolean; warning: string | null }
function call<T>(name: string, args?: Record<string, unknown>): Promise<T> { return isTauri() ? (args ? invoke<T>(name, args) : invoke<T>(name)) : Promise.reject(new Error('browser')); }
export const tts_state = () => call<TtsView>('tts_state');
export const tts_skip = () => call<void>('tts_skip');
export const tts_clear = () => call<void>('tts_clear');
export const tts_test = (text: string) => call<void>('tts_test', { text });
export const obs_add_tts = () => call<void>('obs_add_tts');
export const onTts = (callback: (view: TtsView) => void) => isTauri() ? listen<TtsView>('tts', e => callback(e.payload)) : Promise.resolve(() => {});
