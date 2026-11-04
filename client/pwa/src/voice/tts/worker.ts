/**
 * 音声合成 Worker。onnxruntime-web（WebGPU）で Irodori-TTS を動かし、区切った音声を返す。
 * GPU は 1 つなので合成は届いた順に 1 件ずつ処理する（前の文の再生中に次の文を作る）。
 */
import * as ort from 'onnxruntime-web/webgpu'
import wasmUrl from 'onnxruntime-web-dist/ort-wasm-simd-threaded.asyncify.wasm?url'
import { IrodoriTts, type EncoderSession, type SpeakerCondition, type TtsConfig } from './pipeline'
import { clearStoredModels, fetchManifest, isStored, loadModelFile, MODEL_BASE, removeStaleVersions, type ModelManifest } from './modelStore'
import type { VoiceSpec, WorkerRequest, WorkerResponse } from './protocol'
import { prepareReference } from './referenceAudio'
import { createTokenizers, type IrodoriTokenizerExtra, type TokenizerJson } from './text'

// tsconfig は DOM 向け（webworker の型と同居できない）なので、使う部分だけ型を付ける。
const scope = self as unknown as { postMessage(message: unknown, transfer: Transferable[]): void; onmessage: ((event: MessageEvent<WorkerRequest>) => void) | null }

// WASM は 1 スレッド（WebGPU が本体なので不要）。複数スレッドにすると blob: URL の Worker を作るため CSP に阻まれる。
ort.env.wasm.numThreads = 1
// 実行時の警告（ノードの割り当てなど）はコンソールを埋めるだけなのでエラーだけ出す。
ort.env.logLevel = 'error'
ort.env.wasm.wasmPaths = { wasm: wasmUrl }

// logSeverityLevel 3 = エラーだけ（読み込み時の割り当て・定数畳み込みの警告で、コンソールが埋まらないように）。
const SESSION_OPTIONS = { executionProviders: ['webgpu'], graphOptimizationLevel: 'all', logSeverityLevel: 3 } satisfies ort.InferenceSession.SessionOptions
const SESSIONS = { cond: 'cond_encoder', duration: 'duration', dit: 'dit_step', decoder: 'dacvae_decoder', speaker: 'speaker_encoder' } as const

let manifest: ModelManifest | undefined
let tts: IrodoriTts | undefined
let loading: Promise<void> | undefined
/** cancel のたびに進める。古い世代の合成は打ち切る。 */
let epoch = 0
type Job =
  | { kind: 'speak'; id: number; text: string; voice: VoiceSpec; seed: number; durationScale: number; epoch: number }
  | { kind: 'clone'; id: number; audio: Float32Array }
  | { kind: 'design'; id: number; caption: string; voiceSeed: number }
const queue: Job[] = []
/** キャプションと声の番号から作った声。返答の全文を同じ声で読むために使い回す。 */
const designed = new Map<string, SpeakerCondition>()
let encoder: Promise<EncoderSession> | undefined

async function speakerFor(engine: IrodoriTts, voice: VoiceSpec): Promise<SpeakerCondition> {
  if (voice.kind === 'clone') return { state: voice.state, mask: voice.mask }
  const key = `${voice.voiceSeed}\u0000${voice.caption.trim()}`
  let speaker = designed.get(key)
  if (!speaker) {
    speaker = await engine.designVoice(voice.caption, voice.voiceSeed)
    designed.set(key, speaker)
  }
  return speaker
}

/** 声のサンプル用エンコーダ（約 55 MB）。初めて使うときに取得して保存する。 */
function loadEncoder(): Promise<EncoderSession> {
  encoder ??= (async () => {
    if (!manifest) throw new Error('音声モデルが読み込まれていません')
    const name = 'dacvae_encoder'
    const model = await loadModelFile(manifest, `${name}.onnx`, () => undefined)
    const data = await loadModelFile(manifest, `${name}.onnx.data`, () => undefined)
    return ort.InferenceSession.create(model, { ...SESSION_OPTIONS, externalData: [{ path: `${name}.onnx.data`, data }] })
  })().catch((error) => { encoder = undefined; throw error })
  return encoder
}
let running = false

const post = (message: WorkerResponse, transfer: Transferable[] = []) => scope.postMessage(message, transfer)
/**
 * onnxruntime-web は WASM 内の C++ 例外を数値（例外オブジェクトのアドレス）のまま投げることがある。
 * GPU やメモリが足りないときに多いので、そう読めるように言い換える。
 */
const errorMessage = (error: unknown) => {
  if (typeof error === 'number') return `onnxruntime の内部エラー（${error}）。GPU のメモリが足りない可能性があります。ほかのタブを閉じるか、ブラウザを再起動してください`
  return error instanceof Error ? error.message : String(error)
}

/** WebGPU と fp16 演算（shader-f16）が使えるか。モデルは fp16 なので両方要る。 */
async function gpuSupport(): Promise<{ ok: true; adapter: string } | { ok: false; reason: string }> {
  const gpu = (navigator as Navigator & { gpu?: { requestAdapter(): Promise<{ features: Set<string>; info?: { vendor?: string; architecture?: string } } | null> } }).gpu
  if (!gpu) return { ok: false, reason: 'このブラウザは WebGPU に対応していません' }
  const adapter = await gpu.requestAdapter().catch(() => null)
  if (!adapter) return { ok: false, reason: 'WebGPU の GPU が見つかりません' }
  if (!adapter.features.has('shader-f16')) return { ok: false, reason: 'GPU が fp16 演算（shader-f16）に対応していません' }
  return { ok: true, adapter: [adapter.info?.vendor, adapter.info?.architecture].filter(Boolean).join(' / ') || 'WebGPU' }
}

async function probe() {
  const support = await gpuSupport()
  try {
    manifest ??= await fetchManifest()
  } catch (error) {
    post({ type: 'probe', supported: false, reason: errorMessage(error), stored: false, totalBytes: 0 })
    return
  }
  const totalBytes = Object.values(manifest.files).reduce((a, b) => a + b, 0)
  post({ type: 'probe', supported: support.ok, reason: support.ok ? undefined : support.reason, stored: await isStored(manifest), totalBytes })
}

async function load() {
  const support = await gpuSupport()
  if (!support.ok) throw new Error(support.reason)
  manifest ??= await fetchManifest()
  await removeStaleVersions()
  const files = manifest.files
  const total = Object.values(files).reduce((a, b) => a + b, 0)
  let loaded = 0
  const onProgress = (bytes: number) => { loaded += bytes; post({ type: 'progress', loaded: Math.min(loaded, total), total }) }
  const sessions = {} as Record<keyof typeof SESSIONS, ort.InferenceSession>
  try {
    // 1 つずつ読む（同時に読むと 1.7 GB がメモリに並ぶ）。
    for (const [key, name] of Object.entries(SESSIONS) as [keyof typeof SESSIONS, string][]) {
      const model = await loadModelFile(manifest, `${name}.onnx`, onProgress)
      const data = await loadModelFile(manifest, `${name}.onnx.data`, onProgress)
      try {
        sessions[key] = await ort.InferenceSession.create(model, { ...SESSION_OPTIONS, externalData: [{ path: `${name}.onnx.data`, data }] })
      } catch (error) {
        console.error(`[tts] ${name} の読み込みに失敗`, error)
        throw new Error(`${name} を GPU に読み込めませんでした: ${errorMessage(error)}`)
      }
    }
    const json = async <T>(name: string): Promise<T> => {
      const res = await fetch(`${MODEL_BASE}${name}`)
      if (!res.ok) throw new Error(`音声モデルを取得できません（${name}: ${res.status}）`)
      return res.json() as Promise<T>
    }
    const [tokenizer, extra, config] = await Promise.all([
      json<TokenizerJson>('tokenizer.json'), json<IrodoriTokenizerExtra>('irodori_tokenizer.json'), json<TtsConfig>('config.json'),
    ])
    const speakerRes = await fetch(`${MODEL_BASE}speaker_noref.f32`)
    if (!speakerRes.ok) throw new Error(`音声モデルを取得できません（speaker_noref.f32: ${speakerRes.status}）`)
    const speaker = { state: new Float32Array(await speakerRes.arrayBuffer()), mask: config.speaker_noref.mask }
    tts = new IrodoriTts(ort, sessions, createTokenizers(tokenizer, extra), config, speaker)
    // 初回は GPU のシェーダーを作るので遅い。短い文で一度通しておく。
    try {
      for await (const _ of tts.synthesize('はい。')) { /* 捨てる */ }
    } catch (error) {
      console.error('[tts] 試しの合成に失敗', error)
      throw new Error(`試しの合成に失敗しました: ${errorMessage(error)}`)
    }
  } catch (error) {
    // 読み込めた分の GPU メモリを解放してから失敗を返す（やり直したときに同じ理由で落ちないように）。
    await Promise.all(Object.values(sessions).map((session) => session.release().catch(() => undefined)))
    tts = undefined
    throw error
  }
  post({ type: 'ready', adapter: support.adapter })
}

async function drain() {
  if (running) return
  running = true
  try {
    for (let job = queue.shift(); job; job = queue.shift()) {
      const current = job
      if (current.kind === 'clone' || current.kind === 'design') {
        try {
          if (!tts) throw new Error('音声モデルが読み込まれていません')
          const speaker = current.kind === 'clone'
            ? await tts.cloneVoice(await loadEncoder(), prepareReference(current.audio))
            : await speakerFor(tts, { kind: 'design', caption: current.caption, voiceSeed: current.voiceSeed })
          // design の結果はキャッシュと共有しているので、複製を渡す。
          const state = speaker.state.slice()
          post({ type: 'speaker', id: current.id, state, mask: speaker.mask }, [state.buffer])
        } catch (error) {
          post({ type: 'error', id: current.id, message: errorMessage(error) })
        }
        continue
      }
      if (!tts || current.epoch !== epoch) { post({ type: 'done', id: current.id }); continue }
      try {
        const speaker = await speakerFor(tts, current.voice)
        if (current.epoch !== epoch) { post({ type: 'done', id: current.id }); continue }
        const chunks = tts.synthesize(current.text, { caption: current.voice.caption, seed: current.seed, speaker, durationScale: current.durationScale, isCancelled: () => current.epoch !== epoch })
        for await (const chunk of chunks) {
          if (current.epoch !== epoch) break
          post({ type: 'chunk', id: current.id, ...chunk }, [chunk.audio.buffer])
        }
      } catch (error) {
        post({ type: 'error', id: current.id, message: errorMessage(error) })
      }
      post({ type: 'done', id: current.id })
    }
  } finally {
    running = false
  }
}

scope.onmessage = (event) => {
  const message = event.data
  switch (message.type) {
    case 'probe':
      probe().catch((error) => post({ type: 'error', message: errorMessage(error) }))
      break
    case 'load':
      loading ??= load().catch((error) => { loading = undefined; post({ type: 'error', message: errorMessage(error) }) })
      break
    case 'speak':
      queue.push({ ...message, kind: 'speak', epoch })
      void drain()
      break
    case 'clone':
      queue.push({ kind: 'clone', id: message.id, audio: message.audio })
      void drain()
      break
    case 'design':
      queue.push({ kind: 'design', id: message.id, caption: message.caption, voiceSeed: message.voiceSeed })
      void drain()
      break
    case 'cancel':
      epoch++
      break
    case 'clear':
      clearStoredModels().then(() => post({ type: 'cleared' }), (error) => post({ type: 'error', message: errorMessage(error) }))
      break
  }
}
