/**
 * 音声チャットの声の保存。サーバー（`/api/client/voices`・本人だけが使える）に保存し、どの端末からでも
 * 使えるようにする。話者条件の本体はこの端末の OPFS にもキャッシュする（毎回取りに行かない）。
 * サンプルの音声そのものはどこにも保存しない（話者条件だけ）。
 */
import { agentGateway, type Voice } from '@/api'
import type { SpeakerCondition } from './tts/pipeline'

export type SavedVoice = Voice

/**
 * 話者条件を作ったモデルの系統。話者エンコーダ（v4.1-Small-MF）が変わったら上げる。
 * 一致しない声は話者条件の意味が違うので使わない。
 */
export const SPEAKER_MODEL = 'irodori-v4.1-small-mf'
const SPEAKER_DIM = 768
const CACHE_DIR = 'voices'
/** 端末だけに保存していた頃（サーバー保存の前）の一覧。見つけたらサーバーへ移す。 */
const LEGACY_INDEX = 'index.json'

async function cacheDir(): Promise<FileSystemDirectoryHandle | undefined> {
  try {
    return await (await navigator.storage.getDirectory()).getDirectoryHandle(CACHE_DIR, { create: true })
  } catch {
    return undefined
  }
}

async function readCache(id: string, bytes: number): Promise<Float32Array | undefined> {
  try {
    const file = await (await (await cacheDir())?.getFileHandle(`${id}.f32`))?.getFile()
    return file && file.size === bytes ? new Float32Array(await file.arrayBuffer()) : undefined
  } catch {
    return undefined
  }
}

async function writeCache(id: string, state: Float32Array) {
  try {
    const handle = await (await cacheDir())?.getFileHandle(`${id}.f32`, { create: true })
    const writable = await handle?.createWritable()
    await writable?.write(new Blob([state as Float32Array<ArrayBuffer>]))
    await writable?.close()
  } catch {
    // キャッシュできなくても、次からサーバーから取り直すだけ。
  }
}

function toBase64(state: Float32Array): string {
  const bytes = new Uint8Array(state.buffer, state.byteOffset, state.byteLength)
  let binary = ''
  for (let i = 0; i < bytes.length; i += 0x8000) binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000))
  return btoa(binary)
}

/** 今のモデルで使える声か。 */
export const usable = (voice: SavedVoice) => voice.speakerModel === SPEAKER_MODEL

export async function listVoices(): Promise<SavedVoice[]> {
  await migrateLegacyVoices()
  return agentGateway.listVoices()
}

/** 声の話者条件（端末にあればそれを、無ければサーバーから取ってキャッシュする）。 */
export async function loadSpeaker(voice: SavedVoice): Promise<SpeakerCondition> {
  const bytes = voice.tokens * SPEAKER_DIM * 4
  let state = await readCache(voice.id, bytes)
  if (!state) {
    state = new Float32Array(await agentGateway.getVoiceState(voice.id))
    if (state.byteLength !== bytes) throw new Error(`声「${voice.name}」のデータが壊れています`)
    await writeCache(voice.id, state)
  }
  return { state, mask: voice.mask }
}

export async function saveVoice(meta: Pick<SavedVoice, 'name' | 'caption' | 'source' | 'seconds'>, speaker: SpeakerCondition): Promise<SavedVoice> {
  const voice = await agentGateway.createVoice({ ...meta, speakerModel: SPEAKER_MODEL, mask: speaker.mask, stateBase64: toBase64(speaker.state) })
  await writeCache(voice.id, speaker.state)
  return voice
}

export const updateVoice = (id: string, patch: Partial<Pick<SavedVoice, 'name' | 'caption'>>) => agentGateway.updateVoice(id, patch)

export async function deleteVoice(id: string): Promise<void> {
  await agentGateway.deleteVoice(id)
  await (await cacheDir())?.removeEntry(`${id}.f32`).catch(() => undefined)
}

type LegacyVoice = { id: string; name: string; caption: string; seconds: number; mask: boolean[] }
let migrating: Promise<void> | undefined

/** サーバー保存の前に端末だけに保存した声を、サーバーへ移す（移せたものは端末の古い形式を消す）。 */
function migrateLegacyVoices(): Promise<void> {
  migrating ??= (async () => {
    const dir = await cacheDir()
    if (!dir) return
    let legacy: LegacyVoice[]
    try {
      legacy = JSON.parse(await (await (await dir.getFileHandle(LEGACY_INDEX)).getFile()).text()) as LegacyVoice[]
    } catch {
      return
    }
    const remaining: LegacyVoice[] = []
    for (const old of legacy) {
      try {
        const file = await (await dir.getFileHandle(`${old.id}.f32`)).getFile()
        await saveVoice({ name: old.name, caption: old.caption, source: 'sample', seconds: old.seconds }, { state: new Float32Array(await file.arrayBuffer()), mask: old.mask })
        await dir.removeEntry(`${old.id}.f32`).catch(() => undefined)
      } catch {
        remaining.push(old)
      }
    }
    if (remaining.length > 0) {
      const writable = await (await dir.getFileHandle(LEGACY_INDEX)).createWritable()
      await writable.write(JSON.stringify(remaining))
      await writable.close()
    } else {
      await dir.removeEntry(LEGACY_INDEX).catch(() => undefined)
    }
  })().finally(() => { migrating = undefined })
  return migrating
}
