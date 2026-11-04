<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { agentGateway, ApiError, ChatReplyTimeoutError, type ChatMessage } from '@/api'
import AppIcon from '@/components/AppIcon.vue'
import MarkdownContent from '@/components/MarkdownContent.vue'
import UiButton from '@/components/UiButton.vue'
import VoiceSampleForm from '@/components/voice/VoiceSampleForm.vue'
import { speechSentences } from '@/voice/replyText'
import { BrowserSpeaker, ChunkPlayer, IrodoriSpeaker, type ReplySpeaker, type SpeechStats } from '@/voice/speaker'
import { listenOnce, speechInputAvailable } from '@/voice/speechInput'
import { irodoriClient } from '@/voice/tts/client'
import type { SpeakerCondition } from '@/voice/tts/pipeline'
import type { VoiceSpec } from '@/voice/tts/protocol'
import type { VoiceSample } from '@/voice/voiceSample'
import { deleteVoice, listVoices, loadSpeaker, saveVoice, updateVoice, usable, type SavedVoice } from '@/voice/voiceStore'

// チャットを音声だけで使う画面。話す → 送信 → 返答を読み上げる、を 1 往復とし、
// ハンズフリーなら読み終えたら次の発話を聞き始める。チャットの履歴はチャット画面と共通。
type Phase = 'idle' | 'listening' | 'thinking' | 'speaking'
type Engine =
  | { kind: 'checking' }
  | { kind: 'needs-download'; totalBytes: number }
  | { kind: 'loading'; loaded: number; total: number }
  | { kind: 'ready'; adapter: string }
  /** `retry` は読み込みの失敗（端末が非対応なのではない）で、やり直せる。 */
  | { kind: 'browser'; reason: string; retry?: boolean }
  | { kind: 'none'; reason: string }

const CAPTION_KEY = 'agent-desk:voice-caption'
const VOICE_SEED_KEY = 'agent-desk:voice-seed'
const HANDS_FREE_KEY = 'agent-desk:voice-hands-free'
const SELECTED_VOICE_KEY = 'agent-desk:voice-selected'
const SPEED_KEY = 'agent-desk:voice-speed'
/** 特徴（キャプション）から作る声を選んでいるときの値。それ以外は保存した声の id。 */
const DESIGNED = 'designed'
/** 話す速さ（上流の duration_scale。大きいほどゆっくり）。 */
const SPEEDS = [{ value: 0.85, label: '速い' }, { value: 0.93, label: 'やや速い' }, { value: 1, label: 'ふつう' }, { value: 1.1, label: 'ややゆっくり' }, { value: 1.2, label: 'ゆっくり' }]
const DEFAULT_CAPTION = '落ち着いた、近い距離感の若い女性話者'
const PREVIEW_TEXT = 'こんにちは。この声で読み上げますね。'
const readStorage = (key: string) => { try { return window.localStorage.getItem(key) } catch { return null } }
const writeStorage = (key: string, value: string) => { try { window.localStorage.setItem(key, value) } catch { /* 保存できなくても動く */ } }

const phase = ref<Phase>('idle')
const engine = ref<Engine>({ kind: 'checking' })
const caption = ref(readStorage(CAPTION_KEY) ?? DEFAULT_CAPTION)
const handsFree = ref(readStorage(HANDS_FREE_KEY) === '1')
// 声の番号。キャプションが同じでも番号ごとに別の声になる（「別の声にする」で進める）。
const voiceSeed = ref(Number(readStorage(VOICE_SEED_KEY) ?? 0) || 0)
const selectedVoice = ref(readStorage(SELECTED_VOICE_KEY) ?? DESIGNED)
const savedVoices = ref<SavedVoice[]>([])
const speakers = new Map<string, SpeakerCondition>()
const speed = ref(Number(readStorage(SPEED_KEY) ?? 1) || 1)
const cloning = ref(false)
const savingDesigned = ref(false)
const previewing = ref(false)
const voicesError = ref('')
const sampleForm = ref<InstanceType<typeof VoiceSampleForm>>()
const heard = ref('')
const reply = ref<ChatMessage>()
const typed = ref('')
const error = ref('')
const slow = ref(false)
const stats = ref<SpeechStats>()
const canListen = speechInputAvailable()

const client = irodoriClient()
const player = new ChunkPlayer()
/** 今選んでいる声。保存した声の話者条件がまだ読めていなければ、特徴から作る声にする。 */
function voiceSpec(): VoiceSpec {
  const saved = savedVoices.value.find((v) => v.id === selectedVoice.value)
  const speaker = saved && speakers.get(saved.id)
  if (saved && speaker) return { kind: 'clone', caption: saved.caption, state: speaker.state, mask: speaker.mask }
  return { kind: 'design', caption: caption.value.trim() || DEFAULT_CAPTION, voiceSeed: voiceSeed.value }
}
const irodori = new IrodoriSpeaker(client, player, () => ({ voice: voiceSpec(), durationScale: speed.value }))
const browser = BrowserSpeaker.available ? new BrowserSpeaker(() => 1 / speed.value) : undefined
let speaker: ReplySpeaker | undefined
let listening: AbortController | undefined
let waiting: AbortController | undefined
let unmounted = false

const gb = (bytes: number) => (bytes / 2 ** 30).toFixed(1)
const engineText = computed(() => {
  const e = engine.value
  switch (e.kind) {
    case 'checking': return '読み上げの準備を確認しています。'
    case 'needs-download': return `この端末で声を作るには、音声モデル（約 ${gb(e.totalBytes)} GB）を一度ダウンロードします。準備ができるまでは標準の読み上げを使います。`
    case 'loading': return `音声モデルを準備しています（${Math.floor((e.loaded / Math.max(e.total, 1)) * 100)}%）。準備ができるまでは標準の読み上げを使います。`
    case 'ready': return `この端末で声を作ります（${e.adapter}）。`
    case 'browser': return `ブラウザ標準の読み上げを使います（${e.reason}）。`
    case 'none': return `読み上げは使えません。返答は画面に表示します（${e.reason}）。`
  }
  return ''
})
const phaseText = computed(() => ({ idle: canListen ? 'マイクを押して話しかけてください' : '下の欄に入力して送信してください', listening: '聞いています…（もう一度押すと送ります）', thinking: slow.value ? '考えています（時間がかかっています）…' : '考えています…', speaking: '話しています（押すと止めて、次を聞きます）' })[phase.value])
const progressPercent = computed(() => (engine.value.kind === 'loading' ? Math.min(100, (engine.value.loaded / Math.max(engine.value.total, 1)) * 100) : 0))
/** マイクの下に出す「今の声」。 */
const currentVoiceLabel = computed(() => {
  const e = engine.value.kind
  if (e === 'browser') return 'ブラウザ標準の読み上げ'
  if (e === 'none') return '読み上げなし（返答は画面に表示します）'
  const saved = savedVoices.value.find((v) => v.id === selectedVoice.value)
  const name = saved ? saved.name : `特徴から作る声（声 ${voiceSeed.value + 1}）`
  return e === 'ready' ? `声: ${name}` : `声: ${name}（準備ができるまでは標準の読み上げ）`
})
const canPreview = computed(() => engine.value.kind === 'ready' && !previewing.value && phase.value === 'idle')

function currentSpeaker(): ReplySpeaker | undefined {
  return engine.value.kind === 'ready' ? irodori : browser
}

async function prepareModel() {
  engine.value = { kind: 'loading', loaded: 0, total: engine.value.kind === 'needs-download' ? engine.value.totalBytes : 1 }
  try {
    const adapter = await client.load((loaded, total) => { engine.value = { kind: 'loading', loaded, total } })
    if (unmounted) return
    engine.value = { kind: 'ready', adapter }
    void client.prepareVoice(voiceSpec()).catch(() => undefined)
  } catch (cause) {
    const reason = cause instanceof Error ? cause.message : String(cause)
    engine.value = browser ? { kind: 'browser', reason: `音声モデルを読み込めませんでした: ${reason}`, retry: true } : { kind: 'none', reason }
  }
}

async function checkEngine() {
  const probe = await client.probe()
  if (!probe.supported) {
    const reason = probe.reason ?? 'この端末では声を作れません'
    engine.value = browser ? { kind: 'browser', reason } : { kind: 'none', reason }
    return
  }
  if (probe.stored) await prepareModel()
  else engine.value = { kind: 'needs-download', totalBytes: probe.totalBytes }
}

function describeError(cause: unknown): string {
  if (cause instanceof ApiError && cause.serverMessage && [400, 409, 429].includes(cause.status ?? 0)) return cause.serverMessage
  if (cause instanceof ChatReplyTimeoutError) return 'エージェントの応答を待ちきれませんでした。チャット画面で履歴を確認してください。'
  if (cause instanceof ApiError && cause.status === 401) return 'ログインの有効期限が切れました。ログインし直してください。'
  if (cause instanceof Error && cause.message) return cause.message
  return 'メッセージを送信できませんでした。'
}

/** 聞き取った（入力した）文を送り、返答を読み上げる。 */
async function exchange(text: string) {
  heard.value = text
  reply.value = undefined
  phase.value = 'thinking'
  slow.value = false
  waiting = new AbortController()
  const message = await agentGateway.sendChatMessage(text, [], { signal: waiting.signal, onTick: ({ elapsedMs }) => { slow.value = elapsedMs >= 20_000 } })
  if (unmounted) return
  reply.value = message
  speaker = currentSpeaker()
  if (!speaker) { phase.value = 'idle'; return }
  phase.value = 'speaking'
  const result = await speaker.speak(speechSentences(message.content))
  if (result) stats.value = result
}

/** 1 往復（聞く → 送る → 読み上げる）。ハンズフリーなら続けて次を聞く。 */
async function turn() {
  error.value = ''
  try {
    for (;;) {
      phase.value = 'listening'
      heard.value = ''
      listening = new AbortController()
      const text = await listenOnce({ signal: listening.signal, onInterim: (t) => { heard.value = t } })
      listening = undefined
      if (unmounted || phase.value !== 'listening') return
      if (!text) { phase.value = 'idle'; return }
      await exchange(text)
      // 読み上げ中に止められた（別の往復が始まった）ら、この往復はここで終える。
      if (unmounted || (phase.value as Phase) !== 'speaking') return
      phase.value = 'idle'
      if (!handsFree.value) return
    }
  } catch (cause) {
    if (unmounted || waiting?.signal.aborted) return
    error.value = describeError(cause)
    phase.value = 'idle'
  }
}

async function onMic() {
  // ユーザー操作の中で音を鳴らせる状態にしておく（自動再生の制限）。
  await player.unlock().catch(() => undefined)
  switch (phase.value) {
    case 'listening': listening?.abort(); return
    case 'thinking': return
    case 'speaking':
      speaker?.stop()
      phase.value = 'idle'
      if (canListen) void turn()
      return
    case 'idle':
      if (canListen) void turn()
  }
}

async function sendTyped() {
  const text = typed.value.trim()
  if (!text || phase.value === 'thinking' || phase.value === 'listening') return
  await player.unlock().catch(() => undefined)
  speaker?.stop()
  typed.value = ''
  error.value = ''
  try {
    await exchange(text)
    if (!unmounted && phase.value === 'speaking') phase.value = 'idle'
  } catch (cause) {
    if (unmounted) return
    error.value = describeError(cause)
    phase.value = 'idle'
  }
}

function saveCaption() {
  writeStorage(CAPTION_KEY, caption.value.trim() || DEFAULT_CAPTION)
  if (engine.value.kind === 'ready') void client.prepareVoice(voiceSpec()).catch(() => undefined)
}
/** 保存した声の話者条件を読み込む（端末のキャッシュか、無ければサーバーから）。 */
async function ensureSpeaker(voice: SavedVoice): Promise<boolean> {
  if (speakers.has(voice.id)) return true
  try {
    speakers.set(voice.id, await loadSpeaker(voice))
    return true
  } catch (cause) {
    voicesError.value = cause instanceof Error ? `声「${voice.name}」を読み込めませんでした: ${cause.message}` : `声「${voice.name}」を読み込めませんでした。`
    return false
  }
}

async function selectVoice() {
  writeStorage(SELECTED_VOICE_KEY, selectedVoice.value)
  const saved = savedVoices.value.find((v) => v.id === selectedVoice.value)
  if (saved && !(await ensureSpeaker(saved))) return
  if (engine.value.kind === 'ready') void client.prepareVoice(voiceSpec()).catch(() => undefined)
}
function saveSpeed() { writeStorage(SPEED_KEY, String(speed.value)) }

async function loadSavedVoices() {
  voicesError.value = ''
  try {
    savedVoices.value = await listVoices()
  } catch {
    voicesError.value = '保存した声の一覧を取得できませんでした。'
    return
  }
  const selected = savedVoices.value.find((v) => v.id === selectedVoice.value)
  if (selected && usable(selected)) await ensureSpeaker(selected)
  else if (selectedVoice.value !== DESIGNED) { selectedVoice.value = DESIGNED; writeStorage(SELECTED_VOICE_KEY, DESIGNED) }
}

/** 作った声を一覧に加えて選び、試しに読む。 */
async function adopt(saved: SavedVoice, speaker: SpeakerCondition) {
  speakers.set(saved.id, speaker)
  savedVoices.value = [...savedVoices.value, saved]
  selectedVoice.value = saved.id
  await selectVoice()
  await preview()
}

function saveError(cause: unknown, what: string): string {
  if (cause instanceof ApiError && cause.serverMessage) return `${what}: ${cause.serverMessage}`
  return cause instanceof Error ? `${what}: ${cause.message}` : `${what}。`
}

/** 声のサンプルから声を作ってサーバーに保存し、その声に切り替えて試しに読む。 */
async function addVoice({ name, caption: style, sample }: { name: string; caption: string; sample: VoiceSample }) {
  cloning.value = true
  error.value = ''
  try {
    // 送ると Worker に移るので、やり直せるよう複製を渡す。
    const speaker = await client.clone(sample.audio.slice())
    const saved = await saveVoice({ name, caption: style, source: 'sample', seconds: Math.round(sample.seconds * 10) / 10 }, speaker)
    sampleForm.value?.reset()
    await adopt(saved, speaker)
  } catch (cause) {
    error.value = saveError(cause, '声を作れませんでした')
  } finally {
    cloning.value = false
  }
}

/** 特徴から作った今の声を保存する（どの端末でも同じ声で読める）。 */
async function saveDesigned() {
  savingDesigned.value = true
  error.value = ''
  try {
    const style = caption.value.trim() || DEFAULT_CAPTION
    const speaker = await client.design(style, voiceSeed.value)
    const name = `${[...style].slice(0, 16).join('')}（声 ${voiceSeed.value + 1}）`
    await adopt(await saveVoice({ name, caption: style, source: 'design', seconds: 0 }, speaker), speaker)
  } catch (cause) {
    error.value = saveError(cause, '声を保存できませんでした')
  } finally {
    savingDesigned.value = false
  }
}

async function renameVoice(voice: SavedVoice) {
  const name = voice.name.trim()
  if (!name) return loadSavedVoices()
  try {
    const updated = await updateVoice(voice.id, { name })
    savedVoices.value = savedVoices.value.map((v) => (v.id === updated.id ? updated : v))
  } catch (cause) {
    voicesError.value = saveError(cause, '名前を変えられませんでした')
  }
}

async function saveVoiceStyle(voice: SavedVoice) {
  try {
    const updated = await updateVoice(voice.id, { caption: voice.caption.trim() })
    savedVoices.value = savedVoices.value.map((v) => (v.id === updated.id ? updated : v))
  } catch (cause) {
    voicesError.value = saveError(cause, '話し方の指定を保存できませんでした')
  }
}

async function removeVoice(voice: SavedVoice) {
  if (!window.confirm(`「${voice.name}」を削除しますか？（ほかの端末からも消えます）`)) return
  try {
    await deleteVoice(voice.id)
  } catch (cause) {
    voicesError.value = saveError(cause, '声を削除できませんでした')
    return
  }
  savedVoices.value = savedVoices.value.filter((v) => v.id !== voice.id)
  speakers.delete(voice.id)
  if (selectedVoice.value === voice.id) { selectedVoice.value = DESIGNED; await selectVoice() }
}
/** 今の声で短い文を読む。 */
async function preview() {
  if (phase.value !== 'idle' || previewing.value) return
  await player.unlock().catch(() => undefined)
  previewing.value = true
  error.value = ''
  try {
    speaker = currentSpeaker()
    await speaker?.speak([PREVIEW_TEXT])
  } catch (cause) {
    error.value = describeError(cause)
  } finally {
    previewing.value = false
  }
}
/** 同じキャプションで別の声にして、試しに読む。 */
async function nextVoice() {
  voiceSeed.value += 1
  writeStorage(VOICE_SEED_KEY, String(voiceSeed.value))
  await preview()
}
function saveHandsFree() { writeStorage(HANDS_FREE_KEY, handsFree.value ? '1' : '0') }
async function clearModel() {
  speaker?.stop()
  await client.clearStored()
  await checkEngine()
}

onMounted(() => {
  void loadSavedVoices()
  checkEngine().catch((cause) => { engine.value = { kind: 'none', reason: describeError(cause) } })
})
onBeforeUnmount(() => {
  unmounted = true
  listening?.abort()
  waiting?.abort()
  speaker?.stop()
})
</script>

<template>
  <section class="voice-page">
    <div class="voice-intro"><span class="muted">話しかけると、返答を声で読み上げます。</span><RouterLink to="/chat" class="text-link">チャットの履歴</RouterLink></div>

    <!-- 読み上げの準備（準備ができていないときだけ目立たせる） -->
    <section v-if="engine.kind !== 'ready'" class="notice surface" :class="engine.kind" role="status">
      <AppIcon :name="engine.kind === 'loading' || engine.kind === 'checking' ? 'downloading' : engine.kind === 'needs-download' ? 'download' : 'info'" />
      <div class="notice-body">
        <p>{{ engineText }}</p>
        <template v-if="engine.kind === 'loading'">
          <div class="progress" :style="{ '--p': `${progressPercent}%` }"></div>
          <p class="muted">{{ gb(engine.loaded) }} / {{ gb(engine.total) }} GB（2 回目からは端末に保存したものを読み込みます）</p>
        </template>
        <div v-if="engine.kind === 'needs-download'" class="row"><UiButton icon="download" @click="prepareModel">音声モデルを準備する</UiButton></div>
        <div v-if="engine.kind === 'browser' && engine.retry" class="row"><UiButton variant="secondary" icon="refresh" @click="prepareModel">もう一度読み込む</UiButton></div>
      </div>
    </section>

    <!-- 会話 -->
    <div class="stage">
      <button class="mic" :class="phase" type="button" :disabled="phase === 'thinking' || (!canListen && phase !== 'speaking')" :aria-label="phaseText" @click="onMic">
        <AppIcon :name="phase === 'speaking' ? 'stop' : phase === 'thinking' ? 'more_horiz' : 'mic'" />
      </button>
      <p class="phase-text" role="status">{{ phaseText }}</p>
      <p class="voice-line muted"><AppIcon name="record_voice_over" />{{ currentVoiceLabel }}</p>
      <label class="toggle"><input v-model="handsFree" type="checkbox" @change="saveHandsFree" /><span>ハンズフリー（読み終えたら続けて聞く）</span></label>
      <p v-if="error" class="error">{{ error }}</p>
    </div>

    <form v-if="!canListen" class="typed" @submit.prevent="sendTyped">
      <p class="muted">このブラウザは音声入力に対応していません。文字で送ると、返答を読み上げます。</p>
      <div class="typed-row"><input v-model="typed" type="text" placeholder="メッセージ" /><UiButton type="submit" icon="send" :disabled="!typed.trim() || phase === 'thinking'">送信</UiButton></div>
    </form>

    <div v-if="heard || reply" class="exchange">
      <div v-if="heard" class="bubble user"><span class="who">あなた</span><p>{{ heard }}</p></div>
      <div v-if="reply" class="bubble agent"><span class="who">エージェント</span><MarkdownContent :source="reply.content" /></div>
    </div>

    <!-- 声 -->
    <section v-if="engine.kind !== 'browser' && engine.kind !== 'none'" class="panel surface">
      <h2 class="section-title">声</h2>
      <div class="voice" :class="{ active: selectedVoice === DESIGNED }">
        <label class="voice-head"><input v-model="selectedVoice" type="radio" :value="DESIGNED" @change="selectVoice" /><span class="voice-name">特徴から作る声</span><span class="muted">声 {{ voiceSeed + 1 }}</span></label>
        <template v-if="selectedVoice === DESIGNED">
          <label class="field"><span>どんな声にするか（例: 落ち着いた、近い距離感の若い女性話者）</span><input v-model="caption" type="text" maxlength="200" @change="saveCaption" /></label>
          <div class="row">
            <UiButton variant="secondary" icon="play_arrow" :disabled="!canPreview" @click="preview">試しに聞く</UiButton>
            <UiButton variant="secondary" icon="refresh" :disabled="!canPreview" @click="nextVoice">別の声にする</UiButton>
            <UiButton variant="secondary" icon="bookmark_add" :disabled="engine.kind !== 'ready' || savingDesigned" @click="saveDesigned">{{ savingDesigned ? '保存しています…' : 'この声を保存' }}</UiButton>
          </div>
        </template>
      </div>

      <p v-if="savedVoices.length > 0" class="group-title">保存した声<span class="muted">（あなただけが、ほかの端末からも使えます）</span></p>
      <div v-for="v in savedVoices" :key="v.id" class="voice" :class="{ active: selectedVoice === v.id, unusable: !usable(v) }">
        <label class="voice-head"><input v-model="selectedVoice" type="radio" :value="v.id" :disabled="!usable(v)" @change="selectVoice" /><span class="voice-name">{{ v.name }}</span><span class="muted">{{ !usable(v) ? '今の音声モデルでは使えません' : v.source === 'sample' ? `${v.seconds} 秒のサンプルから` : '特徴から作った声' }}</span></label>
        <template v-if="selectedVoice === v.id">
          <label class="field"><span>名前</span><input v-model="v.name" type="text" maxlength="40" @change="renameVoice(v)" /></label>
          <label class="field"><span>話し方の指定（任意・例: 明るく、少し早口で）</span><input v-model="v.caption" type="text" maxlength="200" @change="saveVoiceStyle(v)" /></label>
          <div class="row">
            <UiButton variant="secondary" icon="play_arrow" :disabled="!canPreview" @click="preview">試しに聞く</UiButton>
            <UiButton variant="text" icon="delete" @click="removeVoice(v)">削除</UiButton>
          </div>
        </template>
      </div>
      <p v-if="voicesError" class="error">{{ voicesError }}</p>

      <details class="more">
        <summary><AppIcon name="add" />声のサンプルから追加</summary>
        <VoiceSampleForm ref="sampleForm" :disabled="engine.kind !== 'ready'" :busy="cloning" @create="addVoice" />
      </details>
    </section>

    <!-- 読み上げの設定 -->
    <section class="panel surface">
      <h2 class="section-title">読み上げの設定</h2>
      <label class="field"><span>話す速さ</span><select v-model.number="speed" @change="saveSpeed"><option v-for="s in SPEEDS" :key="s.value" :value="s.value">{{ s.label }}</option></select></label>
      <details class="more">
        <summary><AppIcon name="tune" />詳しい情報</summary>
        <div class="details-body">
          <p class="muted">{{ engineText }}</p>
          <p v-if="stats" class="muted">前回の読み上げ: 最初の音まで {{ (stats.firstAudioMs / 1000).toFixed(1) }} 秒・途切れ {{ (stats.stallMs / 1000).toFixed(1) }} 秒・音声 {{ stats.audioSec.toFixed(1) }} 秒</p>
          <div v-if="engine.kind === 'ready'" class="row"><UiButton variant="text" icon="delete" @click="clearModel">この端末に保存した音声モデルを削除</UiButton></div>
        </div>
      </details>
    </section>
  </section>
</template>

<style scoped>
.voice-page{max-width:720px;margin:0 auto;display:flex;flex-direction:column;gap:16px}
.voice-intro{display:flex;justify-content:space-between;gap:12px;align-items:end;border-bottom:1px solid var(--line);padding-bottom:15px}.voice-intro .muted{margin:0}.text-link{font-size:13px;color:var(--primary);white-space:nowrap}
.notice{display:flex;gap:12px;padding:14px 16px;align-items:flex-start}.notice>.app-icon{color:var(--primary);margin-top:2px}.notice.browser>.app-icon,.notice.none>.app-icon{color:var(--text-medium)}.notice-body{flex:1;min-width:0;display:flex;flex-direction:column;gap:10px}.notice-body p{margin:0;font-size:14px}.notice-body .muted{font-size:12px}
.stage{display:flex;flex-direction:column;align-items:center;gap:8px;padding:24px 0 8px;text-align:center}
.mic{width:112px;height:112px;border-radius:50%;border:2px solid var(--primary-border);background:var(--primary-soft);color:var(--primary);display:grid;place-items:center;transition:transform .15s ease,background .15s ease}.mic .app-icon{font-size:48px}.mic:hover:not(:disabled){background:rgba(187,134,252,.2)}.mic:disabled{opacity:.6;cursor:default}
.mic.listening{background:var(--primary);color:#000;animation:pulse 1.4s ease-in-out infinite}.mic.speaking{border-color:var(--primary)}
@keyframes pulse{0%,100%{box-shadow:0 0 0 0 rgba(187,134,252,.45)}50%{box-shadow:0 0 0 16px rgba(187,134,252,0)}}
@media(prefers-reduced-motion:reduce){.mic.listening{animation:none}}
.phase-text{margin:6px 0 0;font-size:16px;font-weight:700}.voice-line{display:flex;align-items:center;gap:6px;margin:0}.voice-line .app-icon{font-size:17px}
.toggle{display:flex;gap:8px;align-items:center;font-size:13px;margin-top:4px;cursor:pointer}.stage .error{margin:4px 0 0}
.typed-row{display:flex;gap:8px}.typed-row input{flex:1;min-width:0;padding:9px 10px;border:1px solid var(--field);border-radius:4px}
.exchange{display:flex;flex-direction:column;gap:10px}.bubble{padding:10px 14px;border-radius:4px;overflow-wrap:anywhere}.bubble p{margin:0}.bubble .who{display:block;font-size:11px;font-weight:700;color:var(--text-medium);margin-bottom:4px}
.bubble.user{align-self:flex-end;max-width:85%;background:var(--primary-soft);border-left:3px solid var(--primary)}.bubble.agent{border:1px solid var(--line);border-left:2px solid var(--line)}
.panel{padding:14px 16px;display:flex;flex-direction:column;gap:10px}.panel .section-title{margin:0 0 2px;font-size:15px}
.group-title{margin:8px 0 0;font-size:13px;font-weight:600}.group-title .muted{font-weight:400;margin-left:4px}
.voice{display:flex;flex-direction:column;gap:10px;padding:10px 12px;border:1px solid var(--line);border-radius:4px}.voice.active{border-color:var(--primary-border);background:var(--primary-soft)}.voice.unusable{opacity:.6}
.voice-head{display:flex;gap:8px;align-items:center;flex-wrap:wrap;cursor:pointer}.voice-name{font-weight:600;font-size:14px}.voice-head .muted{font-size:12px}
.row{display:flex;gap:8px;flex-wrap:wrap;align-items:center}
.field{display:flex;flex-direction:column;gap:5px;font-size:12px;color:var(--text-medium)}.field input{padding:9px 10px;border:1px solid var(--field);border-radius:4px;font-size:14px}.field select{padding:9px 10px;border:1px solid var(--field);border-radius:4px;font-size:14px;max-width:220px}
.more{border-top:1px solid var(--line);padding-top:10px}.more>summary{display:flex;align-items:center;gap:6px;cursor:pointer;font-size:13px;font-weight:600;color:var(--primary);list-style:none}.more>summary::-webkit-details-marker{display:none}.more[open]>summary{margin-bottom:10px}
.details-body{display:flex;flex-direction:column;gap:8px}.details-body .muted{margin:0}
.progress{height:4px;background:var(--line);border-radius:2px;overflow:hidden;position:relative}.progress::after{content:'';position:absolute;inset:0 auto 0 0;width:var(--p);background:var(--primary);transition:width .2s ease}
@media(max-width:640px){.stage{padding:16px 0 4px}.mic{width:96px;height:96px}.voice-intro .muted{display:none}}
</style>
