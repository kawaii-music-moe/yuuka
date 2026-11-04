#!/usr/bin/env python3
"""Write the browser tokenizer files for the PWA (client/pwa/src/voice/tts/text.ts).

  tokenizer.json          the modernbert-ja tokenizer as shipped with the checkpoint (captions)
  irodori_tokenizer.json  BOS id + the vocab ids RareTokenSplitter disables + its forced splits
                          (reading text; mirrors split_rare_tokens=True in the runtime)

With --corpus, also writes reference encodings of a mixed corpus to check the TS tokenizer.

  python make_tokenizer.py artifacts/onnx [--corpus artifacts/tokenizer_corpus.json]
"""
from __future__ import annotations

import argparse
import json
import random
from pathlib import Path

from transformers import AutoTokenizer

from common import DEFAULT_REPO
from irodori_tts.inference_runtime import download_hf_checkpoint
from irodori_tts.text_normalization import normalize_text
from irodori_tts.token_split import RareTokenSplitter


def corpus() -> list[str]:
    base = [
        "おはようございます！今日の東京は午後から雨の予報です。", "セブンイレブンで浦和レッズのチケットを買った😊",
        "代表取締役社長の山田です。東日本大震災から十年。", "Hello, world. 3.14 は円周率…", "ｶﾀｶﾅ と ＡＢＣ と ①②",
        "絵文字🫠🦊🍣と記号♪★→※", "  前後に 空白  ", "改行\nを含む\n文", "〜ですね〜", "「括弧」で（囲む）",
        "𠮷野家で𩸽を食べた", "تجربة عربية", "한국어 테스트", "ﾃｽﾄ‼️⁉️", "tab\tと　全角空白",
    ]
    rng = random.Random(0)
    pool = "あいうえおかきくけこアイウエオ漢字東京駅前日本語。、！？ーabcXYZ0123 😊🫠🦊♪★→…「」（）\n𠮷"
    return base + ["".join(rng.choice(pool) for _ in range(rng.randint(1, 40))) for _ in range(400)]


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("out_dir")
    p.add_argument("--corpus", help="also write reference ids for the TS tokenizer test")
    args = p.parse_args()
    out = Path(args.out_dir)
    out.mkdir(parents=True, exist_ok=True)

    ckpt = Path(download_hf_checkpoint(DEFAULT_REPO))
    tok = AutoTokenizer.from_pretrained(str(ckpt.parent / "tokenizer"))
    splitter = RareTokenSplitter(tok.backend_tokenizer)
    original = json.loads(tok.backend_tokenizer.to_str())
    split = json.loads(splitter.tokenizer.to_str())
    disabled = [i for i, (a, b) in enumerate(zip(original["model"]["vocab"], split["model"]["vocab"])) if a[1] != b[1]]

    (out / "tokenizer.json").write_text(tok.backend_tokenizer.to_str())
    extra = {"bos_token_id": tok.bos_token_id, "disabled": disabled, "forced": {str(k): v for k, v in splitter.forced.items()}}
    (out / "irodori_tokenizer.json").write_text(json.dumps(extra))
    print(f"[tokenizer] bos={tok.bos_token_id} disabled={len(disabled)} forced={len(splitter.forced)}")

    if args.corpus:
        rows = []
        for s in corpus():
            norm = normalize_text(s).strip()
            rows.append({"raw": s, "norm": norm, "text_ids": splitter.encode(norm) if norm else [],
                         "caption_ids": tok.encode(s, add_special_tokens=False)})
        Path(args.corpus).write_text(json.dumps(rows, ensure_ascii=False))
        print(f"[corpus] {len(rows)} rows -> {args.corpus}")


if __name__ == "__main__":
    main()
