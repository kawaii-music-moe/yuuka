/**
 * サーバーでの音声生成（GPU の無い端末向け）。yuuka 本体が `/api/client/tts/*` をログイン済みの要求だけ
 * 音声生成サーバー（`client/pwa/server/tts`）へ中継する。音声は 16 bit PCM（48 kHz モノラル）で区切りごとに
 * 届くので、届いた分から鳴らす。
 */
import type { ReplySpeaker, SpeechStats } from './speaker'
import type { ChunkPlayer } from './speaker'
import type { SpeakerCondition } from './tts/pipeline'
import type { VoiceSpec } from './tts/protocol'
import { REFERENCE_SAMPLE_RATE } from './tts/referenceAudio'

const BASE = '/api/client/tts'
/** サーバーに送る声のサンプルの上限（秒）。16 bit で約 5.8 MB（本体の本文上限 10 MB に収まる）。 */
export const SERVER_SAMPLE_MAX_SECONDS = 60
/** これだけ溜まったら鳴らす（細かすぎる区切りで再生が途切れないように）。 */
const MIN_PLAY_SAMPLES = REFERENCE_SAMPLE_RATE / 4

async function post(path: string, body: unknown, signal?: AbortSignal): Promise<Response> {
  return fetch(`${BASE}/${path}`, { method: 'POST', credentials: 'include', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body), signal })
}

async function failure(res: Response, fallback: string): Promise<Error> {
  try {
    const body = (await res.json()) as { message?: string }
    if (body.message) return new Error(body.message)
  } catch { /* 本文なし */ }
  return new Error(`${fallback}（${res.status}）`)
}

function toBase64(bytes: Uint8Array): string {
  let binary = ''
  for (let i = 0; i < bytes.length; i += 0x8000) binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000))
  return btoa(binary)
}

function fromBase64(text: string): Uint8Array {
  return Uint8Array.from(atob(text), (c) => c.charCodeAt(0))
}

const speakerFromWire = (body: { stateBase64: string; mask: boolean[] }): SpeakerCondition => {
  const bytes = fromBase64(body.stateBase64)
  return { state: new Float32Array(bytes.buffer, bytes.byteOffset, bytes.byteLength / 4), mask: body.mask }
}

/** サーバーでの音声生成が使えるか（設定されていて、音声生成サーバーが動いているか）。 */
export async function serverTtsAvailable(): Promise<boolean> {
  try {
    return (await fetch(`${BASE}/status`, { credentials: 'include' })).ok
  } catch {
    return false
  }
}

/** キャプションと声の番号から作る声の話者条件（保存用）。 */
export async function serverDesign(caption: string, voiceSeed: number): Promise<SpeakerCondition> {
  const res = await post('design', { caption, voiceSeed })
  if (!res.ok) throw await failure(res, 'サーバーで声を作れませんでした')
  return speakerFromWire(await res.json())
}

/** 声のサンプル（48 kHz モノラル）から話者条件を作る。長いサンプルは先頭 60 秒だけ使う。 */
export async function serverClone(mono48k: Float32Array): Promise<SpeakerCondition> {
  const samples = mono48k.subarray(0, SERVER_SAMPLE_MAX_SECONDS * REFERENCE_SAMPLE_RATE)
  const pcm = new Int16Array(samples.length)
  for (let i = 0; i < samples.length; i++) pcm[i] = Math.round(Math.max(-1, Math.min(1, samples[i])) * 0x7fff)
  const res = await post('clone', { pcmBase64: toBase64(new Uint8Array(pcm.buffer)) })
  if (!res.ok) throw await failure(res, 'サーバーで声を作れませんでした')
  return speakerFromWire(await res.json())
}

type WireVoice = { kind: 'design'; caption: string; voiceSeed: number } | { kind: 'clone'; caption: string; key: string; stateBase64?: string; mask?: boolean[] }

function wireVoice(voice: VoiceSpec, withState: boolean): WireVoice {
  if (voice.kind === 'design') return voice
  const key = voice.key ?? 'unsaved'
  if (!withState) return { kind: 'clone', caption: voice.caption, key }
  return { kind: 'clone', caption: voice.caption, key, stateBase64: toBase64(new Uint8Array(voice.state.buffer, voice.state.byteOffset, voice.state.byteLength)), mask: voice.mask }
}

/**
 * このページを開いてから、サーバーに話者条件を渡した声の key。知らない声は最初から話者条件を付けて送り、
 * 以降は key だけ送る（サーバーが再起動して忘れていれば 428 が返るので、付けて送り直す）。
 */
const sentToServer = new Set<string>()

/** サーバーで作った音声を読み上げる。文ごとに要求し、届いた音声から順に鳴らす。 */
export class ServerSpeaker implements ReplySpeaker {
  private controller?: AbortController

  constructor(private readonly player: ChunkPlayer, private readonly voice: () => { voice: VoiceSpec; durationScale: number }) {}

  async speak(sentences: string[]): Promise<SpeechStats | undefined> {
    this.controller?.abort()
    const controller = new AbortController()
    this.controller = controller
    this.player.stallSec = 0
    const started = performance.now()
    let firstAudioMs = -1
    const stats: SpeechStats['sentences'] = sentences.map((text) => ({ text, audioSec: 0 }))
    const { voice, durationScale } = this.voice()
    try {
      for (const [i, text] of sentences.entries()) {
        const seconds = await this.speakOne(text, voice, i, durationScale, controller.signal, () => { if (firstAudioMs < 0) firstAudioMs = performance.now() - started })
        stats[i].audioSec = seconds
      }
      await this.player.whenIdle()
    } catch (error) {
      if (controller.signal.aborted) return undefined
      throw error
    }
    if (controller.signal.aborted || firstAudioMs < 0) return undefined
    return { firstAudioMs, stallMs: this.player.stallSec * 1000, audioSec: stats.reduce((a, s) => a + s.audioSec, 0), sentences: stats }
  }

  /** 1 文を生成して鳴らす。保存した声をサーバーが覚えていなければ、話者条件を付けて送り直す。 */
  private async speakOne(text: string, voice: VoiceSpec, seed: number, durationScale: number, signal: AbortSignal, onFirst: () => void): Promise<number> {
    const key = voice.kind === 'clone' ? voice.key ?? 'unsaved' : undefined
    let res = await post('speak', { text, voice: wireVoice(voice, key !== undefined && !sentToServer.has(key)), seed, durationScale }, signal)
    if (res.status === 428) res = await post('speak', { text, voice: wireVoice(voice, true), seed, durationScale }, signal)
    if (!res.ok || !res.body) throw await failure(res, 'サーバーで音声を作れませんでした')
    if (key) sentToServer.add(key)
    const rate = Number(res.headers.get('x-sample-rate')) || REFERENCE_SAMPLE_RATE
    const reader = res.body.getReader()
    let pending = new Uint8Array(0)
    let buffered: Float32Array[] = []
    let bufferedSamples = 0
    let total = 0
    const flush = () => {
      if (bufferedSamples === 0) return
      const audio = new Float32Array(bufferedSamples)
      let o = 0
      for (const part of buffered) { audio.set(part, o); o += part.length }
      buffered = []
      bufferedSamples = 0
      onFirst()
      this.player.enqueue(audio, rate)
    }
    for (;;) {
      const { value, done } = await reader.read()
      if (done) break
      // 16 bit 境界で切れていない分は次に回す。
      const bytes = new Uint8Array(pending.length + value.length)
      bytes.set(pending)
      bytes.set(value, pending.length)
      const usable = bytes.length - (bytes.length % 2)
      pending = bytes.slice(usable)
      const view = new DataView(bytes.buffer, 0, usable)
      const part = new Float32Array(usable / 2)
      for (let i = 0; i < part.length; i++) part[i] = view.getInt16(i * 2, true) / 0x8000
      buffered.push(part)
      bufferedSamples += part.length
      total += part.length
      if (bufferedSamples >= MIN_PLAY_SAMPLES) flush()
    }
    flush()
    return total / rate
  }

  stop() {
    this.controller?.abort()
    this.player.stop()
  }
}
