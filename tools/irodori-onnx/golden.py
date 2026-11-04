#!/usr/bin/env python3
"""PyTorch reference outputs for the JS/ONNX parity test (no watermark, no_ref + caption)."""
import json
import re
import sys
from pathlib import Path

import numpy as np
import torch

from common import DEFAULT_REPO
from irodori_tts.inference_runtime import InferenceRuntime, RuntimeKey, SamplingRequest, download_hf_checkpoint
from irodori_tts.rf import draw_initial_noise

out = Path(sys.argv[1] if len(sys.argv) > 1 else "artifacts/golden")
out.mkdir(parents=True, exist_ok=True)
CASES = [
    {"text": "おはようございます！今日の東京は午後から雨の予報です。", "caption": "落ち着いた、近い距離感の若い女性話者", "seed": 1234},
    {"text": "十時から定例ミーティング、そのあと十四時に歯医者の予約が入っています。", "caption": "", "seed": 7},
]

torch.set_num_threads(4)
rt = InferenceRuntime.from_key(RuntimeKey(checkpoint=download_hf_checkpoint(DEFAULT_REPO), model_device="cpu", codec_device="cpu"))
rt.watermarker.model = None
meta = []
for i, case in enumerate(CASES):
    res = rt.synthesize(SamplingRequest(text=case["text"], caption=case["caption"] or None, no_ref=True, seed=case["seed"]))
    frames = next(int(m) for msg in res.messages for m in re.findall(r"using_frames=(\d+)", msg))
    noise = draw_initial_noise(case["seed"], (1, frames, rt.model_cfg.patched_latent_dim), device=torch.device("cpu"), dtype=torch.float32)[0]
    noise.numpy().astype(np.float32).tofile(out / f"case{i}_noise.f32")
    audio = res.audio.float().reshape(-1).numpy()
    audio.astype(np.float32).tofile(out / f"case{i}_audio.f32")
    meta.append({**case, "frames": frames, "samples": int(audio.shape[0]), "timings": dict(res.stage_timings)})
    print(json.dumps(meta[-1], ensure_ascii=False), flush=True)
(out / "cases.json").write_text(json.dumps(meta, ensure_ascii=False, indent=2))
