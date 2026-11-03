import { isTauri } from '@tauri-apps/api/core';
import { defaultSettings, type Settings, type WidgetState } from './api';
import type { TtsView } from './ttsApi';
import type { Profile, Source, Render } from './voiceApi';

export const mockEnabled = !isTauri() && import.meta.env.DEV;
let settings: Settings | undefined;

export async function mockCall<T>(name: string, args?: Record<string, unknown>): Promise<T> {
  const currentSettings = () => settings ??= structuredClone(defaultSettings);
  let result: unknown;
  switch (name) {
    case 'twitch_status': result = false; break;
    case 'spotify_status': result = { connected: false, premium: null }; break;
    case 'obs_check': result = ''; break;
    case 'get_settings': result = currentSettings(); break;
    case 'save_settings': {
      const saved = structuredClone(args?.s as Settings);
      for (const action of [saved.rules.music, saved.rules.tts]) {
        for (const trigger of action.triggers) {
          if (!trigger.id) trigger.id = `t${saved.rules.next_id++}`;
        }
      }
      settings = saved;
      result = saved;
      break;
    }
    case 'rule_errors': result = typeof window !== 'undefined' && new URLSearchParams(window.location.search).get('mock') === 'errors'
      ? { 'tts-reward': 'reward_create_failed' } : {}; break;
    case 'get_state': result = {
      visible: true, now: { id: 1, track: { title: 'Перемен', artist: 'Кино', duration_ms: 294000 }, requester: 'night_owl' },
      queue: ['Спокойная ночь', 'Звезда по имени Солнце', 'Группа крови'].map((title, i) => ({ id: i + 2, track: { title, artist: 'Кино', duration_ms: 240000 }, requester: ['alice', 'boris', 'luna'][i] })),
      progress_ms: 78000, mode: 'Playing', warning: null
    } satisfies WidgetState; break;
    case 'running_port': result = currentSettings().port; break;
    case 'spotify_volume': result = 65; break;
    case 'list_themes': result = ['underground', 'glass', 'crt', 'block'].map(id => ({ id, name: id, author: '', width: 800, height: 200 })); break;
    case 'tts_state': result = {
      current: { id: 1, user: 'alice', text: 'Хорошего стрима!', reward: true, synthesizing: false },
      queue: ['Привет всем в чате', 'Спасибо за музыку'].map((text, i) => ({ id: i + 2, user: ['boris', 'luna'][i], text, reward: i === 0, synthesizing: false })),
      widget: true, warning: null
    } satisfies TtsView; break;
    case 'vl_status': result = { installed: true, encoderInstalled: true, ffmpeg: true }; break;
    case 'vl_profiles': result = ['Анна', 'Михаил', 'Рассказчик'].map((name, i): Profile => ({ id: i + 1, name, tags: ['ru'], styleSha256: `mock-${i}`, origin: i === 2 ? 'clone' : 'builtin', recipe: null, meta: {}, createdAt: 1793700000000 })); break;
    case 'vl_sources': result = ['Голос у микрофона', 'Чтение рассказа'].map((name, i): Source => ({ id: i + 1, name, durationMs: 18000 + i * 4000, consent: true, createdAt: 1793700000000 })); break;
    case 'vl_renders': result = ['Привет! Это проверка русского синтеза.', 'Добро пожаловать на стрим.', 'Спасибо за поддержку!'].map((text, i): Render => ({ id: i + 1, profileId: i + 1, recipe: null, text, lang: 'ru', speed: 1, quality: 5, durationMs: 3200, wallMs: 1800, favorite: i === 0, createdAt: 1793700000000 })); break;
    case 'vl_jobs': case 'vl_train_runs': result = []; break;
    case 'vl_train_status': result = { installing: false, languages: ['ru', 'en'], uv: null, uvOk: false, pack: 'none', gpu: null, torchCuda: null, diskFreeMb: 64000 }; break;
    case 'vl_peaks': result = Array.from({ length: 120 }, (_, i) => 0.15 + Math.abs(Math.sin(i * 1.7)) * 0.65); break;
  }
  return structuredClone(result) as T;
}
