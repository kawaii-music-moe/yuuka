#!/usr/bin/env python3
"""Export Irodori-TTS v4.1-Small-MF to ONNX graphs for an onnxruntime-web pipeline.

Graphs (dynamic batch / sequence dims; graph IO is always fp32):
  cond_encoder : ModernBERT backbone + text and caption projectors.
                 (input_ids, mask) -> (text_state, caption_state). The JS side runs it
                 once for the reading text and once (cached) for the voice caption.
  duration     : duration predictor -> log frames.
  dit_step     : one MeanFlow velocity evaluation (no context KV cache).
  speaker_encoder : reference latent (already patched by speaker_patch_size) -> speaker
                 condition. The PWA designs a voice once from the caption and clones it from
                 the generated latent, so every sentence of a reply keeps the same voice.
--part speaker writes config.json and speaker_noref.f32 (the speaker condition the
runtime builds for no_ref=True).

Each export checks parity against PyTorch with onnxruntime (CPU). Run one part per
process to keep peak memory low (the DiT export peaks around 5 GB):
  python export.py --part cond|duration|dit|speaker --out-dir artifacts/onnx
  python export.py --part cond|dit|speaker_encoder --fp16 --out-dir artifacts/onnx_fp16
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path

import numpy as np
import torch
import torch.nn.functional as F
from torch.export import Dim

from common import load_model
from export_patches import apply_export_patches, reset_rope_caches


def project(proj, state: torch.Tensor, mask: torch.Tensor) -> torch.Tensor:
    """PretrainedConditionProjector.forward with the backbone output precomputed."""
    out = proj.projector(state)
    if proj.projector_type == "residual_mlp":
        out = out + proj.residual_down(F.silu(proj.residual_up(proj.residual_norm(state))))
    return out * mask.unsqueeze(-1).to(out.dtype)


class Wrapper(torch.nn.Module):
    """Graph IO stays fp32; `h` casts activations to the model dtype (fp16 export)."""

    def __init__(self, model):
        super().__init__()
        self.m = model
        self.dtype = next(model.parameters()).dtype

    def h(self, x):
        return x.to(self.dtype)


class CondEncoder(Wrapper):
    def forward(self, input_ids, mask):
        state = self.m.pretrained_text_backbone(input_ids, mask)
        text_state = self.m.text_norm(project(self.m.text_encoder, state, mask))
        caption_state = self.m.caption_norm(project(self.m.caption_encoder, state, mask))
        return text_state.float(), caption_state.float()


class Duration(Wrapper):
    def forward(self, text_state, text_mask, speaker_state, speaker_mask, caption_state, caption_mask,
                duration_features, has_speaker, has_caption):
        return self.m.predict_duration_log_frames(
            text_state=self.h(text_state), text_mask=text_mask,
            speaker_state=self.h(speaker_state), speaker_mask=speaker_mask,
            caption_state=self.h(caption_state), caption_mask=caption_mask,
            duration_features=self.h(duration_features), has_speaker=has_speaker, has_caption=has_caption,
        ).float()


class DiTStep(Wrapper):
    def forward(self, x_t, t, delta_t, text_state, text_mask, speaker_state, speaker_mask,
                caption_state, caption_mask):
        # t / delta_t stay fp32: the timestep embedding is computed before the cast.
        return self.m.forward_with_encoded_conditions(
            x_t=self.h(x_t), t=t, delta_t=delta_t,
            text_state=self.h(text_state), text_mask=text_mask,
            speaker_state=self.h(speaker_state), speaker_mask=speaker_mask,
            caption_state=self.h(caption_state), caption_mask=caption_mask,
            latent_mask=None, context_kv_cache=None,
        ).float()


class SpeakerEncoder(Wrapper):
    def forward(self, ref_patched, ref_mask):
        # The mean token is computed in fp32: onnxruntime-web's WebGPU Clip (clamp_min of the
        # mask count) fails to compile its shader with fp16 inputs.
        state = self.m.speaker_norm(self.m.speaker_encoder(self.h(ref_patched), ref_mask)).float()
        return self.m._prepend_masked_mean_token(state, ref_mask)


def masks(b, n, valid):
    m = torch.zeros(b, n, dtype=torch.bool)
    for i, v in enumerate(valid):
        m[i, :v] = True
    return m


def export(module, inputs: dict, dynamic: dict, outputs: list[str], path: Path, opset: int):
    path.parent.mkdir(parents=True, exist_ok=True)
    names = list(inputs)
    torch.onnx.export(
        module, tuple(inputs.values()), str(path),
        input_names=names, output_names=outputs,
        dynamic_shapes={k: dynamic.get(k) for k in names},
        opset_version=opset, dynamo=True, external_data=True,
    )
    size = sum(f.stat().st_size for f in path.parent.glob(path.name + "*")) / 1e6
    print(f"[onnx] {path} ({size:.0f} MB)", flush=True)


def parity(module, path: Path, cases: list[dict], outputs: list[str]):
    import onnxruntime as ort

    sess = ort.InferenceSession(str(path), providers=["CPUExecutionProvider"])
    for i, feed in enumerate(cases):
        with torch.inference_mode():
            ref = module(*feed.values())
        ref = ref if isinstance(ref, tuple) else (ref,)
        got = sess.run(outputs, {k: v.numpy() for k, v in feed.items()})
        for name, r, g in zip(outputs, ref, got):
            r = r.float().numpy()
            print(f"[parity case{i}] {name} shape={g.shape} max_abs={np.abs(r - g).max():.3e} "
                  f"ref_absmax={np.abs(r).max():.3e}", flush=True)


def speaker_noref(model, cfg):
    """Speaker condition exactly as the runtime builds it for no_ref=True."""
    ref_len = max(1, int(cfg.speaker_patch_size))
    ref_latent = torch.zeros(1, ref_len, cfg.latent_dim * cfg.latent_patch_size)
    ref_mask = torch.zeros(1, ref_len, dtype=torch.bool)
    ids = torch.tensor([[1, 2]])
    with torch.inference_mode():
        _, _, sp_state, sp_mask, _, _ = model.encode_conditions(
            text_input_ids=ids, text_mask=torch.ones_like(ids, dtype=torch.bool),
            ref_latent=ref_latent, ref_mask=ref_mask,
            caption_input_ids=ids, caption_mask=torch.ones_like(ids, dtype=torch.bool),
        )
    return sp_state.float().contiguous(), sp_mask


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--part", required=True, choices=["cond", "duration", "dit", "speaker", "speaker_encoder"])
    p.add_argument("--out-dir", default="artifacts/onnx")
    p.add_argument("--opset", type=int, default=18)
    p.add_argument("--fp16", action="store_true", help="export from a half() model (fp32 graph IO)")
    args = p.parse_args()
    out = Path(args.out_dir)

    model, cfg, train_cfg, ckpt = load_model()
    apply_export_patches()
    reset_rope_caches(model)
    sp_state, sp_mask = speaker_noref(model, cfg)
    if args.fp16:
        model = model.half()
    b = Dim("b")
    torch.manual_seed(0)

    if args.part == "speaker":
        state, mask = speaker_noref(model, cfg)
        out.mkdir(parents=True, exist_ok=True)
        state.numpy().astype(np.float32).tofile(out / "speaker_noref.f32")
        meta = {
            "speaker_noref": {"shape": list(state.shape), "mask": mask[0].tolist()},
            "max_text_len": int(train_cfg.get("max_text_len", 256)),
            "max_caption_len": int(train_cfg.get("max_caption_len", 512)),
            "latent_dim": cfg.latent_dim, "latent_patch_size": cfg.latent_patch_size,
            "patched_latent_dim": cfg.patched_latent_dim, "text_dim": cfg.text_dim,
            "caption_dim": cfg.caption_dim, "speaker_dim": cfg.speaker_dim,
            "duration_aux_dim": cfg.duration_aux_dim, "sample_rate": 48000, "hop_length": 1920,
            "num_steps": 4, "text_add_bos": cfg.text_add_bos, "caption_add_bos": cfg.caption_add_bos_resolved,
            "speaker_patch_size": cfg.speaker_patch_size,
        }
        (out / "config.json").write_text(json.dumps(meta, indent=2))
        print(json.dumps(meta), flush=True)
        return

    if args.part == "cond":
        mod = CondEncoder(model).eval()
        s = Dim("s")
        ex = {"input_ids": torch.randint(5, 1000, (2, 24)), "mask": masks(2, 24, [24, 17])}
        dyn = {"input_ids": {0: b, 1: s}, "mask": {0: b, 1: s}}
        outs = ["text_state", "caption_state"]
        path = out / "cond_encoder.onnx"
        export(mod, ex, dyn, outs, path, args.opset)
        parity(mod, path, [ex, {"input_ids": torch.randint(5, 1000, (1, 61)), "mask": masks(1, 61, [61])}], outs)

    if args.part == "duration":
        mod = Duration(model).eval()
        st, sc, sk = Dim("st"), Dim("sc"), Dim("sk")

        def case(bsz, lt, lc, lt_valid, lc_valid):
            return {
                "text_state": torch.randn(bsz, lt, cfg.text_dim), "text_mask": masks(bsz, lt, lt_valid),
                "speaker_state": sp_state.expand(bsz, -1, -1).contiguous(),
                "speaker_mask": sp_mask.expand(bsz, -1).contiguous(),
                "caption_state": torch.randn(bsz, lc, cfg.caption_dim), "caption_mask": masks(bsz, lc, lc_valid),
                "duration_features": torch.rand(bsz, cfg.duration_aux_dim),
                "has_speaker": torch.zeros(bsz, dtype=torch.bool),
                "has_caption": torch.ones(bsz, dtype=torch.bool),
            }

        ex = case(2, 24, 16, [24, 20], [16, 9])
        dyn = {"text_state": {0: b, 1: st}, "text_mask": {0: b, 1: st},
               "speaker_state": {0: b, 1: sk}, "speaker_mask": {0: b, 1: sk},
               "caption_state": {0: b, 1: sc}, "caption_mask": {0: b, 1: sc},
               "duration_features": {0: b}, "has_speaker": {0: b}, "has_caption": {0: b}}
        outs = ["log_frames"]
        path = out / "duration.onnx"
        export(mod, ex, dyn, outs, path, args.opset)
        parity(mod, path, [ex, case(1, 40, 22, [40], [22])], outs)

    if args.part == "dit":
        mod = DiTStep(model).eval()
        s, st, sc, sk = Dim("s"), Dim("st"), Dim("sc"), Dim("sk")

        def case(bsz, ls, lt, lc, lt_valid, lc_valid):
            return {
                "x_t": torch.randn(bsz, ls, cfg.patched_latent_dim),
                "t": torch.rand(bsz), "delta_t": torch.full((bsz,), 0.25),
                "text_state": torch.randn(bsz, lt, cfg.text_dim), "text_mask": masks(bsz, lt, lt_valid),
                "speaker_state": sp_state.expand(bsz, -1, -1).contiguous(),
                "speaker_mask": sp_mask.expand(bsz, -1).contiguous(),
                "caption_state": torch.randn(bsz, lc, cfg.caption_dim), "caption_mask": masks(bsz, lc, lc_valid),
            }

        ex = case(2, 92, 24, 16, [24, 20], [16, 9])
        dyn = {"x_t": {0: b, 1: s}, "t": {0: b}, "delta_t": {0: b},
               "text_state": {0: b, 1: st}, "text_mask": {0: b, 1: st},
               "speaker_state": {0: b, 1: sk}, "speaker_mask": {0: b, 1: sk},
               "caption_state": {0: b, 1: sc}, "caption_mask": {0: b, 1: sc}}
        outs = ["v"]
        path = out / "dit_step.onnx"
        export(mod, ex, dyn, outs, path, args.opset)
        parity(mod, path, [ex, case(1, 150, 40, 22, [40], [22])], outs)


    if args.part == "speaker_encoder":
        mod = SpeakerEncoder(model).eval()
        t = Dim("t")
        width = cfg.latent_dim * cfg.latent_patch_size * cfg.speaker_patch_size

        def case(bsz, n, valid):
            return {"ref_patched": torch.randn(bsz, n, width), "ref_mask": masks(bsz, n, valid)}

        ex = case(2, 30, [30, 21])
        dyn = {"ref_patched": {0: b, 1: t}, "ref_mask": {0: b, 1: t}}
        outs = ["speaker_state", "speaker_mask"]
        path = out / "speaker_encoder.onnx"
        export(mod, ex, dyn, outs, path, args.opset)
        parity(mod, path, [ex, case(1, 52, [52])], outs)


if __name__ == "__main__":
    main()
