(() => {
  const params = new URLSearchParams(location.search);
  const part = ['now', 'queue'].includes(params.get('part')) ? params.get('part') : 'all';
  const value = Number.parseFloat(params.get('scale'));
  const scale = Number.isFinite(value) ? Math.min(4, Math.max(0.5, value)) : 1;
  document.documentElement.dataset.part = part;
  document.documentElement.style.zoom = String(scale);
  const listeners = new Map();
  window.StreamAssist = { on(name, fn) {
    if (!['state', 'nowPlaying', 'queue', 'progress', 'hidden'].includes(name)) return;
    if (!listeners.has(name)) listeners.set(name, []);
    listeners.get(name).push(fn);
  }};
  function emit(name, value) { for (const fn of listeners.get(name) || []) fn(value); }
  let role = part !== 'queue' && typeof window.obsstudio !== 'undefined' ? 'player' : 'view';
  let socket, player, itemId, started = false, pending = [];
  function report(event, extra = {}) {
    if (itemId != null && socket?.readyState === WebSocket.OPEN)
      socket.send(JSON.stringify({ type: 'yt', item_id: itemId, event, ...extra }));
  }
  function command(name, data) {
    if (name === 'YtPlay') {
      itemId = data.item_id; started = false;
      player.mute();
      player.loadVideoById({ videoId: data.video_id, startSeconds: data.start_ms / 1000 });
    } else if (name === 'YtUnmute') player.unMute();
    else if (name === 'YtStop') { player.stopVideo(); itemId = null; }
    else if (name === 'YtPause') player.pauseVideo();
    else if (name === 'YtResume') player.playVideo();
  }
  if (role === 'player') {
    let container = document.getElementById('sa-youtube');
    if (!container) {
      container = document.createElement('div'); container.id = 'sa-youtube';
      Object.assign(container.style, { position: 'fixed', left: '-300px', top: '0', width: '200px', height: '200px' });
      document.body.append(container);
    }
    window.onYouTubeIframeAPIReady = () => {
      const instance = new YT.Player('sa-youtube', { width: 200, height: 200,
        playerVars: { autoplay: 1, playsinline: 1 }, events: {
          onReady() { player = instance; for (const [name, data] of pending) command(name, data); pending = []; },
          onStateChange(e) {
            if (e.data === YT.PlayerState.PLAYING && !started) {
              started = true; report('started', { duration_ms: Math.round(instance.getDuration() * 1000) });
            } else if (e.data === YT.PlayerState.ENDED) report('ended');
          },
          onError() { report('error'); }
        }
      });
    };
    const script = document.createElement('script'); script.src = 'https://www.youtube.com/iframe_api'; document.head.append(script);
  }
  function connect() {
    socket = new WebSocket(`ws://${location.host}/ws?role=${role}`);
    socket.onmessage = ({ data }) => {
      const message = JSON.parse(data);
      if (message === 'Reload') { location.reload(); return; }
      if (message.State) {
        const state = message.State;
        emit('state', state); emit('nowPlaying', state.now); emit('queue', state.queue);
        emit('progress', state.progress_ms); emit('hidden', !state.visible);
      } else if (role === 'player') {
        const name = typeof message === 'string' ? message : Object.keys(message)[0];
        const value = message[name];
        if (player) command(name, value); else pending.push([name, value]);
      }
    };
    socket.onclose = (event) => {
      if (event.code === 4001) {
        role = 'view'; pending = []; itemId = null;
        if (player) { player.mute(); player.stopVideo(); }
      }
      setTimeout(connect, 2000);
    };
  }
  connect();
})();
