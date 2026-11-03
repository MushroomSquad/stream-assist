# Copied from Saganaki22/supertonic_embeddings_trainer@84a6f4c220cd7cc57dce14f638d43e13fa777f42 (MIT). Changes: none.
import os
import json
from datetime import datetime


def save_style(path, style_ttl, style_dp, source_file=None):
    style_json = {
        "style_ttl": {
            "data": style_ttl.detach().cpu().numpy().tolist(),
            "dims": [1, 50, 256],
            "type": "float32",
        },
        "style_dp": {
            "data": style_dp.detach().cpu().numpy().tolist(),
            "dims": [1, 8, 16],
            "type": "float32",
        },
        "metadata": {
            "source_file": source_file or "unknown",
            "source_sample_rate": 44100,
            "target_sample_rate": 44100,
            "extracted_at": datetime.now().isoformat(),
        },
    }
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        json.dump(style_json, f)
