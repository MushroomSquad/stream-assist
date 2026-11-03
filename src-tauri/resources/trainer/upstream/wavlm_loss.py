# Copied from Saganaki22/supertonic_embeddings_trainer@84a6f4c220cd7cc57dce14f638d43e13fa777f42 (MIT). Changes: retained only WavLM layer and target/loss functions and required imports.
import torch
import torch.nn.functional as F
import torchaudio

WAVLM_LAYER = 3


def extract_wavlm_targets(wavlm, wav_tensor, device, sr=44100):
    wav_16k = torchaudio.functional.resample(
        wav_tensor.unsqueeze(0).to(device), sr, 16000
    )
    with torch.no_grad():
        out = wavlm(wav_16k, output_hidden_states=True)
    feat = out.hidden_states[WAVLM_LAYER]
    return feat.mean(dim=1).detach(), feat.std(dim=1).detach()


def wavlm_loss(wavlm, gen_wav, target_features, device, sr=44100):
    wav_16k = torchaudio.functional.resample(gen_wav.unsqueeze(0), sr, 16000)
    out = wavlm(wav_16k, output_hidden_states=True)
    feat = out.hidden_states[WAVLM_LAYER]
    tgt_mean, tgt_std = target_features
    return (
        F.mse_loss(feat.mean(dim=1), tgt_mean)
        + F.mse_loss(feat.std(dim=1), tgt_std)
    )
