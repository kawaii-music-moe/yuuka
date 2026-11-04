"""Monkeypatches that make Irodori-TTS v4.1 exportable with torch.onnx (dynamo).

All patches are numerically equivalent to the upstream code on the paths we export
(CPU / SDPA, no FlashAttention):

- RoPE: upstream uses torch.view_as_complex / view_as_real, which the exporter cannot
  lower. Replaced with the same rotation in real arithmetic. Adapted from
  ngc-shj/irodori-tts-webgpu (MIT, Copyright (c) 2026 NOGUCHI Shoji).
- _safe_attention_mask: drops the data-dependent `if has_any.all()` branch (the
  duration predictor's pooling hits it).
- build_context_attention_plan: upstream calls int(query_len), which pins the DiT
  latent length to the example input. Only the SDPA path is exported, and it needs
  the concatenated key mask alone.
"""
from __future__ import annotations

import torch

import irodori_tts.attention as A
import irodori_tts.model as M


def precompute_freqs_cis_real(dim: int, end: int, theta: float = 10000.0) -> torch.Tensor:
    # Rotation angles of shape (end, dim/2) instead of complex exponentials.
    freqs = 1.0 / (theta ** (torch.arange(0, dim, 2, dtype=torch.float32) / dim))
    t = torch.arange(end, dtype=torch.float32)
    return torch.outer(t, freqs)


def apply_rotary_emb_real(x: torch.Tensor, freqs: torch.Tensor) -> torch.Tensor:
    # x: (B, S, H, Dh); freqs: (S, Dh/2) angles. Consecutive pairs (x[2i], x[2i+1]) are
    # (real, imag). Strided slicing (no -1 reshape) keeps every dim symbolic.
    cos = torch.cos(freqs)[None, :, None, :]
    sin = torch.sin(freqs)[None, :, None, :]
    xf = x.float()
    a = xf[..., 0::2]
    b = xf[..., 1::2]
    out = torch.flatten(torch.stack([a * cos - b * sin, a * sin + b * cos], dim=-1), start_dim=-2)
    return out.type_as(x)


def _rope_freqs_dynamic(self, seq_len, device):
    # No cache and no data-dependent branch, so seq_len stays symbolic.
    return precompute_freqs_cis_real(self.head_dim, seq_len).to(device)


def _safe_attention_mask_branchless(x: torch.Tensor, mask: torch.Tensor):
    # Rows with no valid key are zeroed and attend to index 0, like upstream.
    mask = mask.to(device=x.device, dtype=torch.bool)
    has_any = mask.any(dim=1)
    x = x * has_any[:, None, None].to(x.dtype)
    first = torch.zeros_like(mask)
    first[:, 0] = True
    return x, mask | (first & ~has_any[:, None])


def _build_context_attention_plan_symbolic(segment_masks, *, query_len, attention_dtype):
    kv_mask = segment_masks[0] if len(segment_masks) == 1 else torch.cat(segment_masks, dim=1)
    return A.ContextAttentionPlan(kv_mask=kv_mask, query_len=query_len, use_fa3=False)


def apply_export_patches() -> None:
    M.precompute_freqs_cis = precompute_freqs_cis_real
    M.apply_rotary_emb = apply_rotary_emb_real
    M.TextEncoder._rope_freqs = _rope_freqs_dynamic
    M.ReferenceLatentEncoder._rope_freqs = _rope_freqs_dynamic
    M.TextToLatentRFDiT._rope_freqs = _rope_freqs_dynamic
    M._safe_attention_mask = _safe_attention_mask_branchless
    M.build_context_attention_plan = _build_context_attention_plan_symbolic


def reset_rope_caches(model: torch.nn.Module) -> None:
    # Drop complex-valued RoPE caches so the patched real precompute runs.
    for module in model.modules():
        if hasattr(module, "_freqs_cis_cache"):
            module._freqs_cis_cache = torch.empty(0, 0, dtype=torch.float32)
