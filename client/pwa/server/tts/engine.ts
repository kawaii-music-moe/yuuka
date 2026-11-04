/**
 * サーバーで Irodori-TTS を動かす（GPU の無い端末向け）。推論はブラウザと同じ `src/voice/tts/` のコードを、
 * onnxruntime-node（CPU）で動かす。モデルは最初の要求で読み込み、しばらく使われなければ解放する
 * （同じホストで yuuka 本体も動いているため、メモリを持ち続けない）。CPU は 1 つの生成で使い切るので、
 * 要求は 1 件ずつ順に処理する。
 */
import { readFile } from 'node:fs/promises'
import path from 'node:path'
import * as ortNode from 'onnxruntime-node'
import type * as OrtWeb from 'onnxruntime-web'
import { IrodoriTts, type AudioChunk, type SpeakerCondition, type TtsConfig, type TtsSessions } from '../../src/voice/tts/pipeline'
import { prepareReference } from '../../src/voice/tts/referenceAudio'
import { createTokenizers, type IrodoriTokenizerExtra, type TokenizerJson } from '../../src/voice/tts/text'

// pipeline.ts は onnxruntime-web の型で書いてある。onnxruntime-node は同じ API（onnxruntime-common）を持つ。
const ort = ortNode as unknown as typeof OrtWeb

const SESSIONS = { cond: 'cond_encoder', duration: 'duration', dit: 'dit_step', decoder: 'dacvae_decoder', speaker: 'speaker_encoder' } as const

export type EngineOptions = { modelDir: string; threads: number; idleMs: number; log: (message: string) => void }

type Loaded = { tts: IrodoriTts; sessions: ortNode.InferenceSession[]; encoder?: ortNode.InferenceSession }

export class ServerTtsEngine {
  private loaded?: Promise<Loaded>
  private queue: Promise<unknown> = Promise.resolve()
  private idleTimer?: ReturnType<typeof setTimeout>
  private readonly designed = new Map<string, SpeakerCondition>()

  constructor(private readonly options: EngineOptions) {}

  get modelVersion() { return path.basename(this.options.modelDir) }
  get isLoaded() { return this.loaded !== undefined }

  /** 要求を 1 件ずつ順に処理する（CPU を取り合わない）。 */
  private exclusive<T>(job: () => Promise<T>): Promise<T> {
    const run = this.queue.then(job, job)
    this.queue = run.catch(() => undefined)
    return run
  }

  private sessionOptions(): ortNode.InferenceSession.SessionOptions {
    // アリーナを切ると、生成のたびに確保・解放する分だけ遅くなるが、使い終えたメモリを OS に返す
    // （同じホストの yuuka 本体と RAM を分け合うため）。
    return { executionProviders: ['cpu'], graphOptimizationLevel: 'all', intraOpNumThreads: this.options.threads, logSeverityLevel: 3, enableCpuMemArena: false, enableMemPattern: false }
  }

  private load(): Promise<Loaded> {
    this.loaded ??= (async () => {
      const started = Date.now()
      const dir = this.options.modelDir
      const sessions = {} as Record<keyof typeof SESSIONS, ortNode.InferenceSession>
      for (const [key, name] of Object.entries(SESSIONS) as [keyof typeof SESSIONS, string][]) {
        sessions[key] = await ortNode.InferenceSession.create(path.join(dir, `${name}.onnx`), this.sessionOptions())
      }
      const json = async <T>(name: string) => JSON.parse(await readFile(path.join(dir, name), 'utf8')) as T
      const [tokenizer, extra, config] = await Promise.all([json<TokenizerJson>('tokenizer.json'), json<IrodoriTokenizerExtra>('irodori_tokenizer.json'), json<TtsConfig>('config.json')])
      const noref = await readFile(path.join(dir, 'speaker_noref.f32'))
      const speaker = { state: new Float32Array(noref.buffer, noref.byteOffset, noref.byteLength / 4).slice(), mask: config.speaker_noref.mask }
      const tts = new IrodoriTts(ort, sessions as unknown as TtsSessions, createTokenizers(tokenizer, extra), config, speaker)
      this.options.log(`model loaded in ${((Date.now() - started) / 1000).toFixed(1)}s (${this.modelVersion}, threads=${this.options.threads})`)
      return { tts, sessions: Object.values(sessions) }
    })().catch((error) => { this.loaded = undefined; throw error })
    return this.loaded
  }

  /** 使われなくなってから idleMs 経ったらモデルを解放する。 */
  private touch() {
    clearTimeout(this.idleTimer)
    this.idleTimer = setTimeout(() => { void this.exclusive(() => this.unload()) }, this.options.idleMs)
    this.idleTimer.unref()
  }

  private async unload() {
    const loaded = await this.loaded?.catch(() => undefined)
    this.loaded = undefined
    this.designed.clear()
    if (!loaded) return
    await Promise.all([...loaded.sessions, ...(loaded.encoder ? [loaded.encoder] : [])].map((s) => s.release().catch(() => undefined)))
    this.options.log('model unloaded (idle)')
  }

  private async designedSpeaker(tts: IrodoriTts, caption: string, voiceSeed: number): Promise<SpeakerCondition> {
    const key = `${voiceSeed}\u0000${caption.trim()}`
    let speaker = this.designed.get(key)
    if (!speaker) {
      speaker = await tts.designVoice(caption, voiceSeed)
      this.designed.set(key, speaker)
    }
    return speaker
  }

  /** 1 文を合成し、区切った音声を `onChunk` に渡す。`speaker` が無ければ caption と voiceSeed で作った声。 */
  speak(
    text: string,
    voice: { caption: string; voiceSeed?: number; speaker?: SpeakerCondition },
    options: { seed: number; durationScale: number; isCancelled: () => boolean },
    onChunk: (chunk: AudioChunk) => void,
  ): Promise<void> {
    return this.exclusive(async () => {
      const { tts } = await this.load()
      try {
        if (options.isCancelled()) return
        const speaker = voice.speaker ?? (await this.designedSpeaker(tts, voice.caption, voice.voiceSeed ?? 0))
        for await (const chunk of tts.synthesize(text, { caption: voice.caption, seed: options.seed, speaker, durationScale: options.durationScale, isCancelled: options.isCancelled })) onChunk(chunk)
      } finally {
        this.touch()
      }
    })
  }

  /** キャプションと声の番号から作る声の話者条件（保存用）。 */
  design(caption: string, voiceSeed: number): Promise<SpeakerCondition> {
    return this.exclusive(async () => {
      try {
        return await this.designedSpeaker((await this.load()).tts, caption, voiceSeed)
      } finally {
        this.touch()
      }
    })
  }

  /** 声のサンプル（48 kHz モノラル）から話者条件を作る。エンコーダは初めて使うときに読み込む。 */
  clone(mono48k: Float32Array): Promise<SpeakerCondition> {
    return this.exclusive(async () => {
      const loaded = await this.load()
      try {
        loaded.encoder ??= await ortNode.InferenceSession.create(path.join(this.options.modelDir, 'dacvae_encoder.onnx'), this.sessionOptions())
        return await loaded.tts.cloneVoice(loaded.encoder as unknown as OrtWeb.InferenceSession, prepareReference(mono48k))
      } finally {
        this.touch()
      }
    })
  }
}
