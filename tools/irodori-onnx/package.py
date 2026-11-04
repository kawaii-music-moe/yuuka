#!/usr/bin/env python3
"""Assemble the model set the PWA downloads from /tts-models/<version>/.

  <dest>/<version>/
    cond_encoder.onnx(.data)    fp16  (export.py --part cond --fp16)
    dit_step.onnx(.data)        fp16  (export.py --part dit --fp16)
    speaker_encoder.onnx(.data) fp16  (export.py --part speaker_encoder --fp16)
    duration.onnx(.data)        fp32  (export.py --part duration; small, kept fp32)
    dacvae_decoder.onnx(.data)  fp16  (noguchis/irodori-tts-onnx, same codec; fetched here)
    dacvae_encoder.onnx(.data)  fp16  (same repo; optional: fetched only when a voice is cloned from a sample)
    config.json, speaker_noref.f32    (export.py --part speaker)
    tokenizer.json, irodori_tokenizer.json   (make_tokenizer.py)
    manifest.json               ONNX files and sizes: `files` are fetched up front, `optional` on demand

The version must match MODEL_VERSION in client/pwa/src/voice/tts/modelStore.ts. Never change
files inside an existing version: they are served with an immutable Cache-Control.

  python package.py --fp32 artifacts/onnx --fp16 artifacts/onnx_fp16 --dest <runtime>/data/tts-models
"""
from __future__ import annotations

import argparse
import json
import shutil
from pathlib import Path

from huggingface_hub import hf_hub_download

VERSION = "irodori-v41-mf-fp16-3"
# fp16 DACVAE codec graphs (decoder with the ConvTranspose -> Conv rewrite onnxruntime-web's WebGPU
# fp16 path needs; encoder converted post-hoc). Same Semantic-DACVAE-Japanese-32dim codec as v4.1.
CODEC_REPO = "noguchis/irodori-tts-onnx"
CODEC_REVISION = "b75a9bbf2c10e12682d37e91e0efaf6d4e54bd29"
ONNX = {"cond_encoder": "fp16", "dit_step": "fp16", "speaker_encoder": "fp16", "duration": "fp32", "dacvae_decoder": "codec"}
OPTIONAL = {"dacvae_encoder": "codec"}


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--fp32", required=True, help="export.py output (duration, config.json, speaker_noref.f32)")
    p.add_argument("--fp16", required=True, help="export.py --fp16 output (cond_encoder, dit_step, speaker_encoder)")
    p.add_argument("--tokenizer", help="make_tokenizer.py output (default: <fp32>/tokenizer)")
    p.add_argument("--dest", required=True)
    p.add_argument("--version", default=VERSION)
    args = p.parse_args()

    out = Path(args.dest) / args.version
    if out.exists():
        raise SystemExit(f"{out} already exists; bump the version instead of overwriting (immutable cache)")
    staging = out.with_name(out.name + ".partial")
    shutil.rmtree(staging, ignore_errors=True)
    staging.mkdir(parents=True)

    src = {"fp32": Path(args.fp32), "fp16": Path(args.fp16)}
    files: dict[str, int] = {}
    optional: dict[str, int] = {}
    for group, sizes in ((ONNX, files), (OPTIONAL, optional)):
        for name, kind in group.items():
            for suffix in (".onnx", ".onnx.data"):
                if kind == "codec":
                    path = Path(hf_hub_download(CODEC_REPO, f"onnx_fp16/{name}{suffix}", revision=CODEC_REVISION))
                else:
                    path = src[kind] / f"{name}{suffix}"
                shutil.copyfile(path, staging / f"{name}{suffix}")
                sizes[f"{name}{suffix}"] = (staging / f"{name}{suffix}").stat().st_size
    tokenizer = Path(args.tokenizer) if args.tokenizer else src["fp32"] / "tokenizer"
    for path in (src["fp32"] / "config.json", src["fp32"] / "speaker_noref.f32",
                 tokenizer / "tokenizer.json", tokenizer / "irodori_tokenizer.json"):
        shutil.copyfile(path, staging / path.name)
    manifest = {"version": args.version, "files": files, "optional": optional}
    (staging / "manifest.json").write_text(json.dumps(manifest, indent=2))
    staging.rename(out)
    total = sum(files.values()) / 2**30
    print(f"[package] {out} ({total:.2f} GiB of ONNX)")


if __name__ == "__main__":
    main()
