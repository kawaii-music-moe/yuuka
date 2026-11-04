/**
 * Irodori-TTS v4.1-Small-MF の推論（onnxruntime-web）。上流 `InferenceRuntime.synthesize` の
 * 「参照音声なし + キャプション + MeanFlow 4 ステップ」経路の移植。
 *
 * 音声はデコーダを区切って順に返す（`synthesize` は AsyncGenerator）。DACVAE デコーダは畳み込み
 * だけで、前後 {@link DECODE_CONTEXT} フレームの文脈を付ければ一括デコードと一致する（誤差 -120 dB
 * 以下を実測）。最初の区切りを短くして早く鳴らし、以降は前の区切りの再生中に間に合うよう倍々に伸ばす。
 */
import type * as Ort from 'onnxruntime-web'
import { durationFeatures, normalizeText, type Tokenizers } from './text'

/** エクスポート時に書き出すモデル設定（`config.json`）。 */
export type TtsConfig = {
  max_text_len: number
  max_caption_len: number
  patched_latent_dim: number
  speaker_dim: number
  duration_aux_dim: number
  sample_rate: number
  hop_length: number
  num_steps: number
  speaker_patch_size: number
  speaker_noref: { shape: number[]; mask: boolean[] }
}

export type TtsSessions = { cond: Ort.InferenceSession; duration: Ort.InferenceSession; dit: Ort.InferenceSession; decoder: Ort.InferenceSession; speaker: Ort.InferenceSession }
/** 声のサンプルを潜在表現にする DACVAE エンコーダ。サンプルから声を作るときだけ読み込む。 */
export type EncoderSession = Ort.InferenceSession
/** 話者条件（話者エンコーダの出力。先頭は全体の平均トークン）。 */
export type SpeakerCondition = { state: Float32Array; mask: boolean[] }
export type StageTimings = { encode: number; duration: number; sample: number; decode: number }
export type AudioChunk = { audio: Float32Array; sampleRate: number; final: boolean; timings?: StageTimings }

const MIN_SECONDS = 0.5
const MAX_SECONDS = 30
/** 区切りデコードで前後に付ける潜在フレーム数（1 フレーム = 40 ms）。 */
export const DECODE_CONTEXT = 8
/** 声を作るときに読ませる見本の文（約 8 秒。参照音声として十分な長さ）。 */
const VOICE_SAMPLE_TEXT = 'こんにちは。今日もよろしくお願いします。わからないことがあれば、いつでも気軽に聞いてくださいね。'
/** 最初の区切り（潜在フレーム数）。0.8 秒分で最初の音を早く出す。 */
const FIRST_CHUNK = 20

/** 区切りの範囲 [start, end)。最初は短く、以降は倍々（直前の区切りの再生中に次をデコードし終える）。 */
export function decodePlan(frames: number, first = FIRST_CHUNK): [number, number][] {
  const plan: [number, number][] = []
  let start = 0
  let size = first
  while (start < frames) {
    // 端数が小さく残るなら最後の区切りに含める（短い呼び出しは文脈のぶん割高）。
    const end = frames - (start + size) < first / 2 ? frames : start + size
    plan.push([start, end])
    start = end
    size *= 2
  }
  return plan
}

/** Echo 方式の末尾トリム: 窓が平坦かつ 0 付近になる最初のフレーム（上流 `find_flattening_point`）。latent は (T, D) 行優先。 */
export function findFlatteningPoint(latent: Float32Array, frames: number, dim: number, windowSize = 20, stdThreshold = 0.05, meanThreshold = 0.1): number {
  const n = windowSize * dim
  for (let i = 0; i < frames; i++) {
    let sum = 0, sq = 0
    // 範囲外は 0 埋め（上流と同じく窓の要素数は常に windowSize * dim）。
    for (let r = i; r < Math.min(i + windowSize, frames); r++) {
      for (let d = 0; d < dim; d++) { const v = latent[r * dim + d]; sum += v; sq += v * v }
    }
    const mean = sum / n
    const std = Math.sqrt(Math.max(sq / n - mean * mean, 0))
    if (std < stdThreshold && Math.abs(mean) < meanThreshold) return i
  }
  return frames
}

/** 決定的な N(0,1) ノイズ（mulberry32 + Box-Muller）。torch の乱数とは一致しない。 */
export function gaussianNoise(n: number, seed: number): Float32Array {
  let a = seed >>> 0
  const rand = () => {
    a = (a + 0x6d2b79f5) >>> 0
    let t = a
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
  const out = new Float32Array(n)
  for (let i = 0; i < n; i += 2) {
    const r = Math.sqrt(-2 * Math.log(rand() || 1e-12))
    const th = 2 * Math.PI * rand()
    out[i] = r * Math.cos(th)
    if (i + 1 < n) out[i + 1] = r * Math.sin(th)
  }
  return out
}

type Caption = { state: Ort.Tensor; mask: Ort.Tensor; has: boolean }

export class IrodoriTts {
  private readonly captions = new Map<string, Caption>()

  constructor(
    private readonly ort: typeof Ort,
    private readonly sessions: TtsSessions,
    private readonly tokenizers: Tokenizers,
    private readonly config: TtsConfig,
    /** 参照音声なしの話者条件（エクスポート時に計算したもの）。声を作るときに使う。 */
    private readonly noReference: SpeakerCondition,
  ) {}

  private tensor(type: 'float32', data: Float32Array, dims: number[]): Ort.Tensor
  private tensor(type: 'int64', data: BigInt64Array, dims: number[]): Ort.Tensor
  private tensor(type: 'bool', data: Uint8Array, dims: number[]): Ort.Tensor
  private tensor(type: 'float32' | 'int64' | 'bool', data: Float32Array | BigInt64Array | Uint8Array, dims: number[]): Ort.Tensor {
    return new this.ort.Tensor(type, data as Float32Array, dims)
  }

  private async encode(ids: BigInt64Array, mask: Uint8Array) {
    return this.sessions.cond.run({
      input_ids: this.tensor('int64', ids, [1, ids.length]),
      mask: this.tensor('bool', mask, [1, mask.length]),
    })
  }

  /** キャプションの条件（声ごとに 1 回だけ計算してキャッシュ）。空なら条件なし（マスク 0）。 */
  private async caption(caption: string): Promise<Caption> {
    const cached = this.captions.get(caption)
    if (cached) return cached
    const { ids, mask } = this.tokenizers.caption.encode(caption, this.config.max_caption_len)
    if (caption === '') mask.fill(0)
    const { caption_state } = await this.encode(ids, mask)
    const value = { state: caption_state, mask: this.tensor('bool', mask, [1, mask.length]), has: caption !== '' }
    this.captions.set(caption, value)
    return value
  }

  /**
   * 文の潜在表現を作る（エンコード・長さ予測・MeanFlow 4 ステップ）。打ち切られたら undefined。
   * 返す `frames` は末尾の無音（平坦になった以降）を除いた長さ。上流は同じ位置で波形を切っている。
   */
  private async latent(text: string, opts: { caption: string; seed: number; speaker: SpeakerCondition; durationScale?: number; noise?: Float32Array; isCancelled: () => boolean }) {
    const cfg = this.config
    const now = () => performance.now()
    let t0 = now()
    const norm = normalizeText(text).trim()
    if (!norm) return undefined
    const { ids, mask } = this.tokenizers.text.encode(norm, cfg.max_text_len)
    const { text_state } = await this.encode(ids, mask)
    const textMask = this.tensor('bool', mask, [1, mask.length])
    const cap = await this.caption(opts.caption.trim())
    const spLen = opts.speaker.mask.length
    const spState = this.tensor('float32', opts.speaker.state, [1, spLen, cfg.speaker_dim])
    const spMask = this.tensor('bool', Uint8Array.from(opts.speaker.mask, Number), [1, spLen])
    const encode = now() - t0
    if (opts.isCancelled()) return undefined

    t0 = now()
    // 長さは参照音声なし（キャプションだけ）の条件で予測する。v4.1-MF の長さ予測は参照音声を渡すと文の長さに
    // ほぼ依らず 3.2〜3.6 秒前後を返し（上流の PyTorch でも同じ）、「はい。」が 3 秒になって余った時間に別の
    // 言葉を話したり、長い文が早口になったりする。参照なしの予測は文の長さに沿う（はい。1.0 秒・了解です。1.4 秒）。
    const noRefLen = this.noReference.mask.length
    const { log_frames } = await this.sessions.duration.run({
      text_state, text_mask: textMask,
      speaker_state: this.tensor('float32', this.noReference.state, [1, noRefLen, cfg.speaker_dim]),
      speaker_mask: this.tensor('bool', Uint8Array.from(this.noReference.mask, Number), [1, noRefLen]),
      caption_state: cap.state, caption_mask: cap.mask,
      duration_features: this.tensor('float32', durationFeatures(norm, ids.length, cfg.max_text_len, false), [1, cfg.duration_aux_dim]),
      has_speaker: this.tensor('bool', Uint8Array.of(0), [1]),
      has_caption: this.tensor('bool', Uint8Array.of(cap.has ? 1 : 0), [1]),
    })
    const fps = cfg.sample_rate / cfg.hop_length
    // durationScale > 1 でゆっくり（上流 `duration_scale`）。
    const predicted = Math.round(Math.expm1((log_frames.data as Float32Array)[0]) * (opts.durationScale ?? 1))
    const S = Math.max(Math.ceil(MIN_SECONDS * fps), Math.min(Math.floor(MAX_SECONDS * fps), predicted))
    const duration = now() - t0
    if (opts.isCancelled()) return undefined

    t0 = now()
    const D = cfg.patched_latent_dim
    let x = opts.noise ?? gaussianNoise(S * D, opts.seed)
    for (let i = 0; i < cfg.num_steps; i++) {
      const t = Math.fround(1 - i / cfg.num_steps)
      const next = Math.fround(1 - (i + 1) / cfg.num_steps)
      const { v } = await this.sessions.dit.run({
        x_t: this.tensor('float32', x, [1, S, D]),
        t: this.tensor('float32', Float32Array.of(t), [1]),
        delta_t: this.tensor('float32', Float32Array.of(t - next), [1]),
        text_state, text_mask: textMask, speaker_state: spState, speaker_mask: spMask,
        caption_state: cap.state, caption_mask: cap.mask,
      })
      const vel = (await v.getData()) as Float32Array
      const nx = new Float32Array(x.length)
      const dt = next - t
      for (let j = 0; j < x.length; j++) nx[j] = x[j] + vel[j] * dt
      x = nx
      if (opts.isCancelled()) return undefined
    }
    const sample = now() - t0
    const flat = findFlatteningPoint(x, S, D)
    return { x, S, frames: flat > 0 ? Math.min(S, flat) : S, timings: { encode, duration, sample } }
  }

  /**
   * キャプションと声の番号（seed）から声を 1 つ作り、その話者条件を返す。
   *
   * 参照音声なしでは、キャプションは声の傾向を決めるだけで、声色は初期ノイズで決まる（文ごとに
   * 別人になる）。そこで見本の文を一度だけ生成し、その潜在表現を参照音声として話者エンコーダに
   * 通す（上流の ref_latent と同じ）。以降の文はこの条件で作るので同じ声になる。
   */
  async designVoice(caption: string, seed: number): Promise<SpeakerCondition> {
    const cfg = this.config
    const made = await this.latent(VOICE_SAMPLE_TEXT, { caption, seed, speaker: this.noReference, isCancelled: () => false })
    if (!made) throw new Error('声を作れませんでした')
    return this.speakerFromLatent(made.x, made.frames)
  }

  /**
   * 声のサンプル（`prepareReference` 済みの 48 kHz モノラル）から話者条件を作る（上流の ref_wav と同じ）。
   * サンプルの音声そのものは残さない（話者条件だけを返す）。
   */
  async cloneVoice(encoder: EncoderSession, wav: Float32Array): Promise<SpeakerCondition> {
    const out = await encoder.run({ wav: this.tensor('float32', wav, [1, 1, wav.length]) })
    const latent = Object.values(out)[0]
    const frames = latent.dims[1]
    if (frames < this.config.speaker_patch_size) throw new Error('声のサンプルが短すぎます')
    return this.speakerFromLatent((await latent.getData()) as Float32Array, frames)
  }

  /** (frames, D) の潜在表現を話者エンコーダに通す。 */
  private async speakerFromLatent(x: Float32Array, frames: number): Promise<SpeakerCondition> {
    const D = this.config.patched_latent_dim
    const p = this.config.speaker_patch_size
    const tokens = Math.floor(frames / p)
    // p フレームずつまとめて (tokens, D * p) にする（上流 patch_sequence_with_mask と同じ並び）。
    const patched = x.slice(0, tokens * p * D)
    const { speaker_state, speaker_mask } = await this.sessions.speaker.run({
      ref_patched: this.tensor('float32', patched, [1, tokens, D * p]),
      ref_mask: this.tensor('bool', new Uint8Array(tokens).fill(1), [1, tokens]),
    })
    return { state: (await speaker_state.getData()) as Float32Array, mask: Array.from((await speaker_mask.getData()) as Uint8Array, Boolean) }
  }

  /**
   * 1 文を合成し、区切りデコードした音声を順に返す。最後の区切りは `final: true` で各段階の時間を持つ。
   * `isCancelled` が true を返すと、次の段階に入る前に打ち切る。`noise` は検証用（上流と同じ初期ノイズを渡す）。
   */
  async *synthesize(text: string, options: { caption?: string; seed?: number; speaker?: SpeakerCondition; durationScale?: number; noise?: Float32Array; isCancelled?: () => boolean } = {}): AsyncGenerator<AudioChunk> {
    const { caption = '', seed = 0, speaker = this.noReference, durationScale, noise, isCancelled = () => false } = options
    const cfg = this.config
    const made = await this.latent(text, { caption, seed, speaker, durationScale, noise, isCancelled })
    if (!made) return
    const { x, S, frames } = made
    const D = cfg.patched_latent_dim
    // (S, D) -> デコーダの入力 (1, D, S)
    const z = new Float32Array(S * D)
    for (let s = 0; s < S; s++) for (let d = 0; d < D; d++) z[d * S + s] = x[s * D + d]
    let decode = 0
    const plan = decodePlan(frames)
    for (const [index, [start, end]] of plan.entries()) {
      if (isCancelled()) return
      const t0 = performance.now()
      const a = Math.max(0, start - DECODE_CONTEXT)
      const b = Math.min(S, end + DECODE_CONTEXT)
      const window = new Float32Array(D * (b - a))
      for (let d = 0; d < D; d++) window.set(z.subarray(d * S + a, d * S + b), d * (b - a))
      const out = await this.sessions.decoder.run({ z: this.tensor('float32', window, [1, D, b - a]) })
      const wav = (await Object.values(out)[0].getData()) as Float32Array
      const audio = wav.slice((start - a) * cfg.hop_length, (end - a) * cfg.hop_length)
      decode += performance.now() - t0
      const final = index === plan.length - 1
      yield { audio, sampleRate: cfg.sample_rate, final, timings: final ? { ...made.timings, decode } : undefined }
    }
  }
}
