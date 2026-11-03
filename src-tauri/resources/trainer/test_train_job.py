"""Adapter contract tests; run with python3 -I -m unittest discover -s this directory."""
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import types
import unittest

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('train_job', HERE / 'train_job.py')
job = importlib.util.module_from_spec(spec)
spec.loader.exec_module(job)


def child(code, *args, **kwargs):
    return subprocess.Popen([sys.executable, '-I', '-c',
                             f"import sys; sys.path.insert(0, {str(HERE)!r}); " + code,
                             *map(str, args)], stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, **kwargs)


class TrainJobTests(unittest.TestCase):
    def test_parse_request_rejects_bad_version(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / 'request.json'
            p.write_text('{"protocolVersion":2}')
            with self.assertRaises(ValueError):
                job.parse_request(p)

    def test_emit_sanitizes_nonfinite(self):
        out = io.StringIO()
        job.emit(out, 'progress', loss=float('nan'), bestLoss=float('inf'), lr=0.1)
        self.assertEqual(json.loads(out.getvalue()),
                         dict(event='progress', loss=None, bestLoss=None, lr=0.1))

    def test_write_atomic_replaces(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / 'state'
            p.write_text('old')
            job.write_atomic(p, lambda tmp: Path(tmp).write_text('new'))
            self.assertEqual(p.read_text(), 'new')
            self.assertFalse(Path(str(p) + '.tmp').exists())

    def test_trim_and_peak(self):
        import numpy as np
        wav = np.array([[0, 0], [.01, .01], [.2, .6], [-.8, -.8], [0, 0]])
        trimmed = job.trim_and_peak(wav)
        np.testing.assert_allclose(trimmed, [.475, -.95])
        np.testing.assert_array_equal(job.trim_and_peak(np.zeros(4)), np.zeros(4))

    def test_texts_16_each(self):
        self.assertEqual(len(set(job.RU_TEXTS)), 16)
        self.assertEqual(len(set(job.EN_TEXTS)), 16)

    def test_error_code_mapping(self):
        class OOM(RuntimeError):
            pass
        torch = types.SimpleNamespace(cuda=types.SimpleNamespace(OutOfMemoryError=OOM))
        self.assertEqual(job.error_code(OOM(), torch), 'GPU_OUT_OF_MEMORY')
        for code in ('GPU_UNAVAILABLE', 'CUDA_DRIVER_MISMATCH',
                     'TRAINING_STATE_CORRUPTED', 'JOB_BUSY'):
            self.assertEqual(job.error_code(job.WorkerError(code), torch), code)
        self.assertEqual(job.error_code(job.WorkerError('FOO'), torch), 'WORKER_FAILED')
        self.assertEqual(job.error_code(RuntimeError('secret'), torch), 'WORKER_FAILED')

    def test_gpu_check_before_import(self):
        # No model, numpy, transformers or real torch imports are needed here.
        for available, expected in [(False, 'GPU_UNAVAILABLE'),
                                    (True, 'CUDA_DRIVER_MISMATCH')]:
            code = f'''import types, train_job as j
sys.modules['torch'] = types.SimpleNamespace(cuda=types.SimpleNamespace(is_available=lambda: {available}), ones=lambda *a, **k: (_ for _ in ()).throw(RuntimeError('no kernel image')))
try:
 j.check_gpu()
except j.WorkerError as e:
 print(e.code)
'''
            p = child(code)
            out, err = p.communicate(timeout=5)
            self.assertEqual(p.returncode, 0, err)
            self.assertEqual(out.decode().strip(), expected)

    def test_eof_triggers_stop(self):
        code = '''import train_job as j, threading
stop = threading.Event()
lock = threading.Lock()
phase = {'loop_started': True, 'step': 0}
j.watch_stdin(sys.stdin, stop, sys.stdout, lock, phase)
assert stop.wait(2)
j.emit(sys.stdout, 'stopped', step=0, stylePath=None)
'''
        p = child(code, stdin=subprocess.PIPE)
        start = time.monotonic()
        out, err = p.communicate(timeout=4)
        self.assertEqual(p.returncode, 0, err)
        self.assertEqual(json.loads(out)['event'], 'stopped')
        self.assertLess(time.monotonic() - start, 5)

    def test_eof_before_loop_emits_stopped(self):
        with tempfile.TemporaryDirectory() as d:
            req = Path(d) / 'request.json'
            req.write_text(json.dumps(dict(protocolVersion=1, upstreamDir=d, runDir=d)))
            code = '''import types, time, train_job as j
class SlowTorch(types.ModuleType):
 def __getattr__(self, name):
  time.sleep(5)
  raise RuntimeError('slow import')
sys.modules['torch'] = SlowTorch('torch')
sys.exit(j.main(sys.argv[1:]))
'''
            p = child(code, req, stdin=subprocess.PIPE)
            out, err = p.communicate(timeout=4)
            self.assertEqual(p.returncode, 0, err)
            self.assertEqual(json.loads(out), dict(event='stopped', step=0, stylePath=None))

    @unittest.skipUnless(os.name == 'posix', 'flock is Unix specific')
    def test_lock_rejects_second_process(self):
        with tempfile.TemporaryDirectory() as d:
            code = '''import train_job as j
try:
 lock = j.acquire_lock(sys.argv[1])
 print('locked', flush=True)
 sys.stdin.read()
except j.WorkerError as e:
 print(e.code, flush=True)
'''
            first = child(code, Path(d) / 'lock', stdin=subprocess.PIPE)
            try:
                self.assertEqual(first.stdout.readline().strip(), b'locked')
                second = child(code, Path(d) / 'lock', stdin=subprocess.DEVNULL)
                out, err = second.communicate(timeout=3)
                self.assertEqual(out.strip(), b'JOB_BUSY', err)
            finally:
                first.communicate(timeout=3)


if __name__ == '__main__':
    unittest.main()
