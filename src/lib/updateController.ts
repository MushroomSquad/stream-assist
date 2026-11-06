export interface UpdateOffer {
  version: string;
  currentVersion: string;
  body?: string;
  download: (progress: (event: DownloadProgress) => void) => Promise<void>;
  install: () => Promise<void>;
  close: () => Promise<void>;
}
export type DownloadProgress =
  | { event: 'Started'; data: { contentLength?: number } }
  | { event: 'Progress'; data: { chunkLength: number } }
  | { event: 'Finished' };
export type UpdatePhase = 'idle' | 'available' | 'downloading' | 'installing' | 'restart';
export interface UpdateState {
  offer: UpdateOffer | null;
  phase: UpdatePhase;
  downloaded: number;
  total: number | undefined;
  error: boolean;
  automatic: boolean;
}
interface Dependencies {
  check: () => Promise<UpdateOffer | null>;
  supported: () => Promise<boolean>;
  prepare: () => Promise<void>;
  recover: () => Promise<void>;
  restart: () => Promise<void>;
  openRelease: (version: string) => Promise<void>;
}

// One check per launch; dismissing affects this launch only. No persistent skip flag.
export function createUpdater(deps: Dependencies, changed: (state: UpdateState) => void) {
  let state: UpdateState = { offer: null, phase: 'idle', downloaded: 0, total: undefined, error: false, automatic: true };
  let checked = false, disposed = false, busy = false, downloaded = false;
  const publish = (patch: Partial<UpdateState>) => {
    state = { ...state, ...patch };
    if (!disposed) changed(state);
  };
  const close = (offer: UpdateOffer | null) => { void offer?.close().catch(() => {}); };
  return {
    async check() {
      if (checked || disposed) return;
      checked = true;
      try {
        const automatic = await deps.supported();
        const offer = await deps.check();
        if (disposed) { close(offer); return; }
        if (offer) publish({ offer, automatic, phase: 'available' });
      } catch { /* Offline, missing manifest or no supported release: startup continues. */ }
    },
    dismiss() {
      if (busy) return;
      close(state.offer);
      publish({ offer: null, phase: 'idle', error: false });
    },
    async update() {
      const offer = state.offer;
      if (!offer || busy || disposed) return;
      busy = true;
      let prepared = false;
      publish({ error: false });
      try {
        if (!state.automatic) {
          await deps.openRelease(offer.version);
          close(offer);
          publish({ offer: null, phase: 'idle' });
          return;
        }
        if (state.phase !== 'restart') {
          if (!downloaded) {
            publish({ phase: 'downloading', downloaded: 0, total: undefined });
            await offer.download(event => {
              if (event.event === 'Started') publish({ total: event.data.contentLength });
              if (event.event === 'Progress') publish({ downloaded: state.downloaded + event.data.chunkLength });
            });
            downloaded = true;
          }
          publish({ phase: 'installing' });
          await deps.prepare();
          prepared = true;
          await offer.install();
          prepared = false;
          publish({ phase: 'restart' });
        }
        await deps.restart();
      } catch {
        if (prepared) await deps.recover().catch(() => {});
        publish({ error: true, phase: state.phase === 'restart' ? 'restart' : 'available' });
      } finally { busy = false; }
    },
    dispose() { disposed = true; close(state.offer); }
  };
}
