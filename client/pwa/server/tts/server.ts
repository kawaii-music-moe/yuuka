/**
 * サーバーでの音声生成（GPU の無い端末向け）の HTTP サーバー。yuuka 本体（Rust）が `/api/client/tts/*` を
 * ログイン済みの要求だけここへ中継する。認証は持たないので、127.0.0.1 でだけ待ち受けること。
 *
 *   TTS_MODEL_DIR=<data/tts-models/<version>> npx tsx server/tts/server.ts
 *
 * - `GET  /status`  → `{ ok, loaded, model }`
 * - `POST /speak`   `{ text, voice, seed, durationScale }` → 16 bit PCM（48 kHz モノラル・リトルエンディアン）を
 *   区切りごとに順に返す（`x-sample-rate`）。保存した声は `voice.key` で覚え、知らない key なら
 *   `voice.stateBase64` を付けて送り直してもらう（428）。
 * - `POST /design`  `{ caption, voiceSeed }` → 話者条件 `{ stateBase64, mask }`
 * - `POST /clone`   `{ pcmBase64 }`（16 bit PCM・48 kHz モノラル）→ 話者条件
 */
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http'
import type { SpeakerCondition } from '../../src/voice/tts/pipeline'
import { ServerTtsEngine } from './engine'
import { decodeSpeaker, encodeSpeaker, floatToPcm16, pcm16ToFloat, parseSpeakRequest, ServerTtsError } from './wire'

const modelDir = process.env.TTS_MODEL_DIR
if (!modelDir) {
  console.error('TTS_MODEL_DIR is required (e.g. data/tts-models/irodori-v41-mf-fp16-3)')
  process.exit(2)
}
const port = Number(process.env.TTS_PORT ?? 3410)
const host = process.env.TTS_HOST ?? '127.0.0.1'
const log = (message: string) => console.log(`[tts] ${message}`)
const engine = new ServerTtsEngine({
  modelDir,
  threads: Number(process.env.TTS_THREADS ?? 3),
  idleMs: Number(process.env.TTS_IDLE_MINUTES ?? 15) * 60_000,
  log,
})
/** 保存した声の話者条件（key は PWA が送る `voiceKey`）。少数だけ覚える。 */
const speakers = new Map<string, SpeakerCondition>()
const MAX_SPEAKERS = 16
/** 声のサンプルは 16 bit で最大 60 秒（= 約 5.8 MB）。base64 と JSON の分を見込む。 */
const MAX_BODY_BYTES = 12 * 1024 * 1024

function remember(key: string, speaker: SpeakerCondition) {
  speakers.delete(key)
  speakers.set(key, speaker)
  while (speakers.size > MAX_SPEAKERS) speakers.delete(speakers.keys().next().value as string)
}

async function readJson(req: IncomingMessage): Promise<unknown> {
  const chunks: Buffer[] = []
  let size = 0
  for await (const chunk of req) {
    size += (chunk as Buffer).length
    if (size > MAX_BODY_BYTES) throw new ServerTtsError(413, '送られたデータが大きすぎます')
    chunks.push(chunk as Buffer)
  }
  try {
    return JSON.parse(Buffer.concat(chunks).toString('utf8'))
  } catch {
    throw new ServerTtsError(400, 'JSON が不正です')
  }
}

function json(res: ServerResponse, status: number, body: unknown) {
  res.writeHead(status, { 'content-type': 'application/json', 'cache-control': 'no-store' })
  res.end(JSON.stringify(body))
}

async function speak(req: IncomingMessage, res: ServerResponse) {
  const request = parseSpeakRequest(await readJson(req))
  let speaker: SpeakerCondition | undefined
  if (request.voice.kind === 'clone') {
    const { key, stateBase64, mask } = request.voice
    if (stateBase64 && mask) remember(key, decodeSpeaker(stateBase64, mask))
    speaker = speakers.get(key)
    if (!speaker) throw new ServerTtsError(428, '声のデータを付けて送り直してください')
  }
  let cancelled = false
  res.on('close', () => { if (!res.writableEnded) cancelled = true })
  res.writeHead(200, { 'content-type': 'application/octet-stream', 'x-sample-rate': '48000', 'cache-control': 'no-store' })
  try {
    await engine.speak(request.text, { caption: request.voice.caption, voiceSeed: request.voice.kind === 'design' ? request.voice.voiceSeed : undefined, speaker }, {
      seed: request.seed, durationScale: request.durationScale, isCancelled: () => cancelled,
    }, (chunk) => { if (!cancelled) res.write(floatToPcm16(chunk.audio)) })
  } catch (error) {
    // 音声を送り始めた後なので、途中で切る（クライアントは届いた分だけ鳴らす）。
    log(`speak failed: ${error instanceof Error ? error.message : String(error)}`)
    res.destroy()
    return
  }
  res.end()
}

const server = createServer((req, res) => {
  const url = new URL(req.url ?? '/', 'http://localhost')
  const handle = async () => {
    if (req.method === 'GET' && url.pathname === '/status') return json(res, 200, { ok: true, loaded: engine.isLoaded, model: engine.modelVersion })
    if (req.method === 'POST' && url.pathname === '/speak') return speak(req, res)
    if (req.method === 'POST' && url.pathname === '/design') {
      const body = (await readJson(req)) as { caption?: unknown; voiceSeed?: unknown }
      if (typeof body.caption !== 'string' || typeof body.voiceSeed !== 'number') throw new ServerTtsError(400, 'caption と voiceSeed が必要です')
      return json(res, 200, encodeSpeaker(await engine.design(body.caption, body.voiceSeed)))
    }
    if (req.method === 'POST' && url.pathname === '/clone') {
      const body = (await readJson(req)) as { pcmBase64?: unknown }
      if (typeof body.pcmBase64 !== 'string') throw new ServerTtsError(400, 'pcmBase64 が必要です')
      const audio = pcm16ToFloat(Buffer.from(body.pcmBase64, 'base64'))
      if (audio.length < 48000 * 3) throw new ServerTtsError(400, '声のサンプルが短すぎます（3 秒以上にしてください）')
      return json(res, 200, encodeSpeaker(await engine.clone(audio)))
    }
    return json(res, 404, { message: 'not found' })
  }
  handle().catch((error) => {
    const status = error instanceof ServerTtsError ? error.status : 500
    if (status === 500) log(`error: ${error instanceof Error ? error.stack : String(error)}`)
    if (res.headersSent) res.destroy()
    else json(res, status, { message: error instanceof ServerTtsError ? error.message : 'サーバーで音声を作れませんでした' })
  })
})
server.requestTimeout = 0
server.listen(port, host, () => log(`listening on http://${host}:${port} (model: ${modelDir})`))
// systemd の停止（SIGTERM）では、受け付け中の要求を待たずに終わる（生成中の文は途中で切れる）。
for (const signal of ['SIGTERM', 'SIGINT'] as const) process.on(signal, () => { log(`${signal}: stopping`); server.close(); process.exit(0) })
