# Irodori-TTS の ONNX 変換（PWA の音声チャット用）

PWA の「音声」画面（`clients/pwa/src/pages/VoicePage.vue`）は、返答の読み上げを利用者の端末の GPU（WebGPU）で生成します。
使うモデルは [Irodori-TTS v4.1-Small-MF](https://huggingface.co/Aratako/Irodori-TTS-v4.1-Small-MF)（MeanFlow 蒸留・4 ステップ）で、
ここにあるスクリプトでブラウザ用の ONNX 一式に変換します。推論の手順は `clients/pwa/src/voice/tts/` に TypeScript で移植してあります。

## 構成

| ファイル | 中身 | 精度 |
|---|---|---|
| `cond_encoder.onnx` | ModernBERT と、読み上げテキスト用・キャプション用の射影（重みは共有） | fp16 |
| `dit_step.onnx` | MeanFlow の 1 ステップ（4 回呼ぶ） | fp16 |
| `speaker_encoder.onnx` | 話者エンコーダ（キャプションで作った見本の声を、返答の全文で使う話者条件にする） | fp16 |
| `duration.onnx` | 長さ予測 | fp32 |
| `dacvae_decoder.onnx` | DACVAE デコーダ（[noguchis/irodori-tts-onnx](https://huggingface.co/noguchis/irodori-tts-onnx) の fp16 版を流用。同じコーデック） | fp16 |
| `dacvae_encoder.onnx` | DACVAE エンコーダ（同上の fp16 版）。声のサンプルから声を作るときだけ取得する（`manifest.json` の `optional`） | fp16 |
| `speaker_noref.f32`・`config.json` | 参照音声なしの話者条件と、モデルの設定 | |
| `tokenizer.json`・`irodori_tokenizer.json` | トークナイザ（読み上げテキストだけ rare-token split をかける） | |

合計は約 1.7 GB です。声はキャプション（「落ち着いた、近い距離感の若い女性話者」など）と声の番号（seed）から見本を一度だけ生成し、
その潜在表現を参照音声として話者エンコーダに通した条件で、返答の全文を読みます（キャプションだけだと声色が初期ノイズで決まり、文ごとに別人の声になるため）。

## 変換の手順

上流の Irodori-TTS を clone し、その uv 環境に ONNX の依存を足して実行します（CPU だけで動きます）。
DiT の変換はメモリを約 5 GB 使うので、1 つずつ実行してください。

```bash
git clone https://github.com/Aratako/Irodori-TTS.git ~/apps/irodori-tts
cd ~/apps/irodori-tts && uv sync --extra cpu && uv pip install onnx onnxscript onnxruntime
export PYTHONPATH=$HOME/apps/irodori-tts:$(pwd -P)/<yuuka>/tools/irodori-onnx
PY=$HOME/apps/irodori-tts/.venv/bin/python
cd <作業ディレクトリ>
$PY .../export.py --part speaker  --out-dir artifacts/onnx
$PY .../export.py --part duration --out-dir artifacts/onnx
$PY .../export.py --part cond --fp16 --out-dir artifacts/onnx_fp16
$PY .../export.py --part dit  --fp16 --out-dir artifacts/onnx_fp16
$PY .../export.py --part speaker_encoder --fp16 --out-dir artifacts/onnx_fp16
$PY .../make_tokenizer.py artifacts/onnx/tokenizer
$PY .../package.py --fp32 artifacts/onnx --fp16 artifacts/onnx_fp16 --dest <実行ディレクトリ>/data/tts-models
```

各 `export.py` は onnxruntime（CPU）で PyTorch との差を表示します。fp32 で最大 6e-5 程度、fp16 で 1e-2 程度が目安です。

`package.py` は `<dest>/<バージョン>/` を作り、`manifest.json`（ONNX ファイルとサイズ）を書きます。
バージョンは `clients/pwa/src/voice/tts/modelStore.ts` の `MODEL_VERSION` と一致させます。
配信は `immutable` キャッシュなので、**既存のバージョンのファイルは書き換えず**、モデルを変えるときはバージョンを上げてください（PWA は古いバージョンの保存分を自動で消します）。

## 配信

yuuka は作業ディレクトリの `data/tts-models/` があれば `/tts-models/*` として配信します（`crates/yuuka-web` の `mount_tts_models`）。
同じ一式を、GPU の無い端末向けのサーバーでの生成（`clients/pwa/server/tts`・`TTS_MODEL_DIR`）でも使います。
どちらも無い場合、PWA の音声画面はブラウザ標準の読み上げに切り替わります。

サーバーでの生成（CPU・i7-7700K の 4 スレッド）では、5.7 秒の文の最初の音まで約 2 秒、全体で約 6 秒（RTF 約 1.1）でした。
CPU 向けに int8 へ量子化すると、メモリは約 1.5 GB に減りますが、DiT だけでも fp32 との相関が 0.86〜0.91 に下がり、
テキストエンコーダでは長さ予測が変わるので使っていません。

## 長さ予測について

v4.1-MF の長さ予測は、参照音声（話者条件）を渡すと文の長さにほぼ依らず 3.2〜3.6 秒前後を返します（上流の PyTorch でも同じ）。
「はい。」が 3 秒になって余った時間に別の言葉を話したり、長い文が早口になったりするので、PWA とサーバーでの生成は、
長さだけを参照音声なし（キャプションだけ）の条件で予測し、生成は話者条件付きで行います。

## 検証の記録（2026-11-04）

- 同じ文・同じ初期ノイズで、PyTorch（上流ランタイム・透かしなし）と波形を比較: fp32 の ORT CPU で相関 1.000、fp16 の WebGPU で 0.996〜0.9993。フレーム数とサンプル数は一致。
- TS のトークナイザは 415 文（絵文字・未知文字のバイト分解・空白・他言語を含む）で Python と完全一致（`make_tokenizer.py --corpus`）。
- デコーダを前後 8 フレームの文脈付きで区切ってデコードすると、一括デコードと一致（-120 dB 以下）。
- 声の揃い方（モデル自身の話者エンコーダでの文同士の類似度、5 文）: キャプションのみ・文ごとに seed を変える 0.83（最小 0.75）、seed を揃える 0.85（0.77）、見本の潜在表現を参照にする 0.95（0.93）。
- 声のサンプルからの声づくり: 11.6 秒のサンプルから、上流（ref_wav）とブラウザ（TS の前処理 + fp16 エンコーダ・WebGPU）で作った話者条件の平均トークンのコサイン類似度 0.995。生成した 5 文とサンプルの類似度は上流 0.922・ブラウザ 0.913。音量の前処理（BS.1770 の積分ラウドネスで -16 LUFS）は audiotools と 0.01 LU 以内で一致。
- Iris Xe（Chrome・WebGPU）で RTF 約 0.55（6 秒の文を 3.3 秒で生成。うちデコード 2.1 秒、DiT 4 ステップ 1.0 秒）。

## ライセンス

Irodori-TTS の重みとコード、Semantic-DACVAE、modernbert-ja はいずれも MIT です。`export_patches.py` の RoPE の置き換えは
[ngc-shj/irodori-tts-webgpu](https://github.com/ngc-shj/irodori-tts-webgpu)（MIT）を元にしています。
上流のランタイムは生成音声に SilentCipher の透かしを入れますが、ブラウザ版では入れていません。
