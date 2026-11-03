"""Offline Supertonic style trainer. Stdout is reserved for JSONL events."""
import json
import math
import os
from pathlib import Path
import sys
import threading
import traceback


RU_TEXTS = [
    'За окном медленно падал снег, укрывая дорожки белым покрывалом.',
    'Утром мы отправились к реке и долго слушали пение птиц.',
    'Расскажи, пожалуйста, какую книгу ты сейчас читаешь.',
    'В старом доме на углу каждую пятницу собираются друзья.',
    'Сегодня прекрасный день для прогулки по тихому парку.',
    'Не бойся задавать вопросы, если хочешь разобраться в сложной задаче.',
    'На кухне пахло свежим хлебом и крепким горячим чаем.',
    'Поезд прибыл вовремя, и путешественники вышли на пустой перрон.',
    'Иногда достаточно одного доброго слова, чтобы поднять настроение.',
    'Мы построили небольшой мост через ручей возле леса.',
    'Почему звёзды кажутся ярче вдали от городских огней?',
    'Положи письмо на стол и закрой за собой дверь.',
    'Ветер качал ветви деревьев, а вдали гремела гроза.',
    'Я помню этот голос с самого раннего детства.',
    'До начала концерта оставалось ещё несколько минут.',
    'Каждое новое утро приносит возможность попробовать снова.',
]

# configs/utterances.py from TRAINER_REV (MIT), copied verbatim.
EN_TEXTS = [
    "I don't like the terms good person or bad person because it's impossible to be entirely good to everyone, or entirely bad to everyone.",
    "Do you always want to live hiding behind the mask you put up for the sake of others? You're you, and there's nothing wrong with that.",
    "People who can't throw something important away can never hope to change anything.",
    'I am the hope of the universe. I am the answer to all living things that cry out for peace.',
    'Those who hate themselves cannot love or trust others.',
    'Fear is not evil. It tells you what your weakness is. And once you know your weakness, you can become stronger as well as kinder.',
    "If you don't like your destiny, don't accept it. Instead, have the courage to change it the way you want it to be.",
    "Fake people have an image to maintain. Real people just don't care.",
    'Reject common sense to make the impossible possible.',
    'Do exactly as you like. That is the true meaning of pleasure.',
    'I hate perfection. To be perfect is to be unable to improve any further.',
    "It's just pathetic to give up on something before you even give it a shot.",
    "You can't sit around envying other people's worlds. You have to go out and change your own.",
    'Being weak is nothing to ashamed of. Staying weak is.',
    'Anything can happen. No one ever thinks it will until it does.',
    'Who decides limits? And based on what? You said you worked hard? Well, maybe you need to work a little harder.',
]


class WorkerError(Exception):
    def __init__(self, code):
        self.code = code
        super().__init__(code)


def parse_request(path):
    with open(path) as f:
        req = json.load(f)
    if req.get('protocolVersion') != 1:
        raise ValueError('unsupported protocol version')
    return req


def emit(proto, event, **fields):
    def sanitize(value):
        if isinstance(value, float) and not math.isfinite(value):
            return None
        if isinstance(value, dict):
            return {k: sanitize(v) for k, v in value.items()}
        if isinstance(value, (list, tuple)):
            return [sanitize(v) for v in value]
        return value
    proto.write(json.dumps(sanitize(dict(event=event, **fields)), allow_nan=False) + '\n')
    proto.flush()


def write_atomic(path, write):
    tmp = str(path) + '.tmp'
    try:
        write(tmp)
        os.replace(tmp, path)
    finally:
        if os.path.exists(tmp):
            os.unlink(tmp)


def acquire_lock(path):
    handle = open(path, 'a+b')
    try:
        if os.name == 'posix':
            import fcntl
            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        elif os.name == 'nt':
            import msvcrt
            handle.write(b'\0')
            handle.flush()
            handle.seek(0)
            msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
    except (BlockingIOError, PermissionError):
        handle.close()
        raise WorkerError('JOB_BUSY') from None
    return handle


def watch_stdin(stream, stop_event, proto, lock, phase):
    def watch():
        while getattr(stream, 'buffer', stream).read(1):
            pass
        stop_event.set()
        timer = threading.Timer(60, os._exit, (1,))
        timer.daemon = True
        timer.start()
        with lock:
            if not phase['loop_started']:
                emit(proto, 'stopped', step=phase['step'], stylePath=None)
                os._exit(0)
    threading.Thread(target=watch, daemon=True).start()


def check_gpu():
    import torch
    if not torch.cuda.is_available():
        raise WorkerError('GPU_UNAVAILABLE')
    try:
        (torch.ones(1, device='cuda') + 1).item()
    except RuntimeError:
        raise WorkerError('CUDA_DRIVER_MISMATCH') from None
    free, total = torch.cuda.mem_get_info()
    torch.cuda.set_per_process_memory_fraction(
        min(0.75, max(0.1, (free - 2 * 1024**3) / total)))
    torch.set_num_threads(4)
    return torch


def error_code(exc, torch=None):
    if torch is not None and isinstance(exc, torch.cuda.OutOfMemoryError):
        return 'GPU_OUT_OF_MEMORY'
    if isinstance(exc, WorkerError) and exc.code in {
        'GPU_UNAVAILABLE', 'GPU_OUT_OF_MEMORY', 'CUDA_DRIVER_MISMATCH',
        'TRAINING_STATE_CORRUPTED', 'JOB_BUSY',
    }:
        return exc.code
    return 'WORKER_FAILED'


def trim_and_peak(wav):
    import numpy as np
    wav = np.asarray(wav, dtype=np.float32)
    if wav.ndim > 1:
        wav = wav.mean(axis=1)
    peak = np.max(np.abs(wav))
    if peak > 0:
        active = np.flatnonzero(np.abs(wav) > peak * 0.1)
        wav = wav[active[0]:active[-1] + 1]
        wav = wav * (0.95 / peak)
    return wav


def find_closest_style(wavlm, model, styles_dir, targets, ids, mask,
                       noisy, lmask, vocoder_steps):
    import torch
    from helper import load_voice_style
    from wavlm_loss import wavlm_loss
    best_loss, closest = float('inf'), None
    with torch.no_grad():
        for path in sorted(Path(styles_dir).glob('[FM]*.json')):
            style = load_voice_style([str(path)])
            ttl = torch.tensor(style.ttl, device='cuda')
            dp = torch.tensor(style.dp, device='cuda')
            wav = model(ids, mask, ttl, vocoder_steps, noisy, lmask).squeeze()
            loss = wavlm_loss(wavlm, wav, targets, 'cuda').item()
            if loss < best_loss:
                best_loss, closest = loss, (ttl, dp)
    if closest is None:
        raise WorkerError('WORKER_FAILED')
    return closest


def train(req, torch, proto, lock, phase, stop_event):
    import numpy as np
    import soundfile as sf
    from transformers import WavLMModel
    from helper import load_text_to_speech, load_voice_style
    from utils_model import SupertonicModel
    from utils_dataloader import get_train_dataloader
    from utils_style import save_style
    from wavlm_loss import extract_wavlm_targets, wavlm_loss

    device = 'cuda'
    run = Path(req['runDir'])
    np.random.seed(req['seed'])
    torch.manual_seed(req['seed'])
    state = None
    if req['resume'] and (run / 'state.pt').exists():
        try:
            state = torch.load(run / 'state.pt', map_location=device, weights_only=True)
            required = {'step', 'best_loss', 'best_ttl', 'best_dp', 'style_ttl',
                        'style_dp', 'noisy_fixed', 'lmask', 'optimizer_state',
                        'scheduler_state', 'total_steps', 'seed'}
            if not required <= state.keys() or state['total_steps'] != req['totalSteps'] or state['seed'] != req['seed']:
                raise ValueError('invalid training state')
            if not isinstance(state['step'], int) or not 0 <= state['step'] <= req['totalSteps']:
                raise ValueError('invalid step')
            with lock:
                phase['step'] = state['step']
        except torch.cuda.OutOfMemoryError:
            raise
        except Exception:
            raise WorkerError('TRAINING_STATE_CORRUPTED') from None

    wav, sr = sf.read(run / 'reference.wav', dtype='float32')
    wav = trim_and_peak(wav)
    sf.write(run / 'reference_prepared.wav', wav, sr)
    texts = [('ru', t) for t in RU_TEXTS] if req['lang'] == 'ru' else [('en', t) for t in EN_TEXTS]
    model_dir = Path(req['modelDir'])
    model = SupertonicModel(str(model_dir / 'onnx'))
    wavlm = WavLMModel.from_pretrained(req['wavlmDir'], local_files_only=True).to(device).eval()
    wavlm.requires_grad_(False)
    targets = extract_wavlm_targets(wavlm, torch.tensor(wav), device, sr=sr)
    tts = load_text_to_speech(str(model_dir / 'onnx'))
    loader = get_train_dataloader(tts, texts)

    def sample(dp):
        ids, mask = next(iter(loader))
        with torch.no_grad():
            duration = (model.dp_model(ids, dp, mask) / 1.05).cpu().numpy()
        noisy, lmask = tts.sample_noisy_latent(duration)
        return torch.tensor(noisy, device=device), torch.tensor(lmask, device=device)

    if state is None:
        initial = load_voice_style([str(model_dir / 'voice_styles/M1.json')])
        noisy, lmask = sample(torch.tensor(initial.dp, device=device))
        ttl, dp = find_closest_style(wavlm, model, model_dir / 'voice_styles',
                                    targets, *next(iter(loader)), noisy, lmask, req['vocoderSteps'])
        noisy, lmask = sample(dp)
        ttl = ttl.detach().clone().requires_grad_(True)
        dp = dp.detach().clone()
        step, best_loss, best_ttl, best_dp = 0, float('inf'), None, dp.clone()
    else:
        try:
            ttl = state['style_ttl'].detach().requires_grad_(True)
            dp, noisy, lmask = (state[k] for k in ('style_dp', 'noisy_fixed', 'lmask'))
            best_ttl, best_dp = state['best_ttl'], state['best_dp']
            step, best_loss = state['step'], state['best_loss']
            for tensor, shape in ((ttl, (1, 50, 256)), (dp, (1, 8, 16)),
                                  (best_ttl, (1, 50, 256)), (best_dp, (1, 8, 16))):
                if tuple(tensor.shape) != shape or not torch.isfinite(tensor).all():
                    raise ValueError('invalid style')
            if not math.isfinite(best_loss) or noisy.shape[0] != 1 or lmask.shape[0] != 1:
                raise ValueError('invalid state')
        except torch.cuda.OutOfMemoryError:
            raise
        except Exception:
            raise WorkerError('TRAINING_STATE_CORRUPTED') from None
    del tts
    optimizer = torch.optim.Adam([ttl], lr=req['lr'])
    scheduler = torch.optim.lr_scheduler.ReduceLROnPlateau(
        optimizer, patience=200, factor=0.5, min_lr=req['lr'] * 0.01)
    if state is not None:
        try:
            optimizer.load_state_dict(state['optimizer_state'])
            scheduler.load_state_dict(state['scheduler_state'])
        except torch.cuda.OutOfMemoryError:
            raise
        except Exception:
            raise WorkerError('TRAINING_STATE_CORRUPTED') from None
    del state
    iterator = iter(loader)
    for _ in range(step % len(texts)):
        next(iterator)

    def save_state():
        data = dict(step=step, best_loss=best_loss, best_ttl=best_ttl, best_dp=best_dp,
                    style_ttl=ttl.detach(), style_dp=dp, noisy_fixed=noisy, lmask=lmask,
                    optimizer_state=optimizer.state_dict(), scheduler_state=scheduler.state_dict(),
                    total_steps=req['totalSteps'], seed=req['seed'])
        write_atomic(run / 'state.pt', lambda tmp: torch.save(data, tmp))

    def style_file(name):
        path = run / name
        write_atomic(path, lambda tmp: save_style(tmp, best_ttl, best_dp))
        return str(path)

    def send(event, **fields):
        with lock:
            emit(proto, event, **fields)

    with lock:
        phase['loop_started'] = True
        emit(proto, 'started', resumedFrom=step)
    while True:
        if stop_event.is_set():
            path = None
            if best_ttl is not None:
                save_state()
                path = style_file('final.json')
            send('stopped', step=step, stylePath=path)
            return
        if step >= req['totalSteps'] or best_loss <= req['threshold']:
            path = style_file('final.json')
            send('done', step=step, stylePath=path, bestLoss=best_loss)
            return
        try:
            ids, mask = next(iterator)
        except StopIteration:
            iterator = iter(loader)
            ids, mask = next(iterator)
        optimizer.zero_grad()
        generated = model(ids, mask, ttl, req['vocoderSteps'], noisy, lmask).squeeze()
        loss = wavlm_loss(wavlm, generated, targets, device)
        loss.backward()
        torch.nn.utils.clip_grad_norm_([ttl], 1.0)
        optimizer.step()
        value = loss.item()
        if not math.isfinite(value):
            raise WorkerError('WORKER_FAILED')
        if value < best_loss:
            best_loss, best_ttl = value, ttl.detach().clone()
        scheduler.step(best_loss)
        step += 1
        if step % 8 == 0:
            send('progress', step=step, total=req['totalSteps'], loss=value,
                 bestLoss=best_loss, lr=optimizer.param_groups[0]['lr'])
        if step % req['saveEvery'] == 0:
            path = style_file(f'ckpt_{step}.json')
            save_state()
            send('checkpoint', step=step, stylePath=path, bestLoss=best_loss)


def main(argv) -> int:
    proto = os.fdopen(os.dup(1), 'w', buffering=1)
    os.dup2(2, 1)
    lock = threading.Lock()
    torch = None
    run_lock = None
    phase = dict(loop_started=False, step=0)
    try:
        req = parse_request(argv[0])
        sys.path[:0] = [os.path.dirname(os.path.abspath(__file__)), req['upstreamDir']]
        if os.name == 'posix':
            os.nice(10)
        run_lock = acquire_lock(Path(req['runDir']) / 'lock')
        stop_event = threading.Event()
        watch_stdin(sys.stdin, stop_event, proto, lock, phase)
        torch = check_gpu()
        train(req, torch, proto, lock, phase, stop_event)
        return 0
    except Exception as exc:
        # Never dump request contents, transcripts, tensors or exception payloads.
        traceback.print_tb(exc.__traceback__, file=sys.stderr)
        with lock:
            phase['loop_started'] = True  # watcher must not follow error with stopped
            emit(proto, 'error', code=error_code(exc, torch))
        return 1
    finally:
        if run_lock is not None:
            run_lock.close()
        proto.close()


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
