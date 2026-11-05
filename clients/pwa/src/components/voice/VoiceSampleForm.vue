<script setup lang="ts">
import { onBeforeUnmount, ref } from 'vue'
import UiButton from '../UiButton.vue'
import { decodeSample, MAX_RECORDING_SECONDS, MIN_SAMPLE_SECONDS, startRecording, type Recording, type VoiceSample } from '@/voice/voiceSample'

// 声のサンプル（音声ファイルか録音）を受け取り、名前と話し方を付けて親に渡す。声を作るのは親（Worker で数秒かかる）。
defineProps<{ disabled: boolean; busy: boolean }>()
const emit = defineEmits<{ create: [payload: { name: string; caption: string; sample: VoiceSample }] }>()

const sample = ref<VoiceSample>()
const source = ref('')
const name = ref('')
const caption = ref('')
const error = ref('')
const recording = ref<Recording>()
const elapsed = ref(0)
const picker = ref<HTMLInputElement>()
let ticker: ReturnType<typeof setInterval> | undefined
let audio: HTMLAudioElement | undefined
let sampleUrl: string | undefined

function accept(next: VoiceSample, label: string, blob: Blob) {
  if (next.seconds < MIN_SAMPLE_SECONDS) {
    error.value = `サンプルが短すぎます（${next.seconds.toFixed(1)} 秒）。${MIN_SAMPLE_SECONDS} 秒以上の音声にしてください。`
    return
  }
  sample.value = next
  source.value = label
  if (sampleUrl) URL.revokeObjectURL(sampleUrl)
  sampleUrl = URL.createObjectURL(blob)
  if (!name.value) name.value = [...label.replace(/\.[^.]+$/, '')].slice(0, 40).join('')
}

async function onFile(event: Event) {
  const file = (event.target as HTMLInputElement).files?.[0]
  if (picker.value) picker.value.value = ''
  if (!file) return
  error.value = ''
  try {
    accept(await decodeSample(file), file.name, file)
  } catch (cause) {
    error.value = cause instanceof Error ? cause.message : String(cause)
  }
}

async function toggleRecording() {
  error.value = ''
  if (recording.value) return finishRecording()
  try {
    recording.value = await startRecording(() => { void finishRecording() })
    elapsed.value = 0
    ticker = setInterval(() => { elapsed.value += 1 }, 1000)
  } catch (cause) {
    error.value = cause instanceof Error ? cause.message : String(cause)
  }
}

async function finishRecording() {
  const current = recording.value
  if (!current) return
  recording.value = undefined
  clearInterval(ticker)
  try {
    const result = await current.stop()
    // 試聴用には、48 kHz に直した録音を WAV にして使う。
    accept(result, '録音した声', pcmBlob(result.audio))
  } catch (cause) {
    error.value = cause instanceof Error ? cause.message : String(cause)
  }
}

/** 試聴用の WAV（16 bit PCM・48 kHz モノラル）。 */
function pcmBlob(samples: Float32Array): Blob {
  const view = new DataView(new ArrayBuffer(44 + samples.length * 2))
  const text = (offset: number, value: string) => [...value].forEach((ch, i) => view.setUint8(offset + i, ch.charCodeAt(0)))
  text(0, 'RIFF'); view.setUint32(4, 36 + samples.length * 2, true); text(8, 'WAVEfmt ')
  view.setUint32(16, 16, true); view.setUint16(20, 1, true); view.setUint16(22, 1, true)
  view.setUint32(24, 48000, true); view.setUint32(28, 96000, true); view.setUint16(32, 2, true); view.setUint16(34, 16, true)
  text(36, 'data'); view.setUint32(40, samples.length * 2, true)
  samples.forEach((v, i) => view.setInt16(44 + i * 2, Math.max(-1, Math.min(1, v)) * 0x7fff, true))
  return new Blob([view], { type: 'audio/wav' })
}

function playSample() {
  if (!sampleUrl) return
  audio?.pause()
  audio = new Audio(sampleUrl)
  void audio.play()
}

function create() {
  if (!sample.value) return
  emit('create', { name: name.value.trim() || '新しい声', caption: caption.value.trim(), sample: sample.value })
}

/** 作り終えたら親が呼ぶ（入力を空に戻す）。 */
function reset() {
  sample.value = undefined
  source.value = ''
  name.value = ''
  caption.value = ''
  audio?.pause()
  if (sampleUrl) URL.revokeObjectURL(sampleUrl)
  sampleUrl = undefined
}
defineExpose({ reset })

onBeforeUnmount(() => {
  recording.value?.cancel()
  clearInterval(ticker)
  audio?.pause()
  if (sampleUrl) URL.revokeObjectURL(sampleUrl)
})
</script>

<template>
  <div class="sample-form">
    <p class="muted">1 人だけがはっきり話している 10〜30 秒の音声がおすすめです。サンプルの音声そのものは保存せず、声の特徴だけを保存します（あなただけが、ほかの端末からも使えます）。本人の同意がない声は使わないでください。</p>
    <div class="row">
      <input ref="picker" type="file" accept="audio/*" hidden @change="onFile" />
      <UiButton variant="secondary" icon="upload_file" :disabled="disabled || !!recording" @click="picker?.click()">音声ファイルを選ぶ</UiButton>
      <UiButton variant="secondary" :icon="recording ? 'stop' : 'mic'" :disabled="disabled" @click="toggleRecording">{{ recording ? `録音を止める（${elapsed} / ${MAX_RECORDING_SECONDS} 秒）` : '録音する' }}</UiButton>
    </div>
    <template v-if="sample">
      <div class="row picked"><span>{{ source }}・{{ sample.seconds.toFixed(1) }} 秒</span><UiButton variant="text" icon="play_arrow" @click="playSample">聞く</UiButton></div>
      <label class="field"><span>名前</span><input v-model="name" type="text" maxlength="40" /></label>
      <label class="field"><span>話し方の指定（任意・空ならサンプルの話し方に近づけます）</span><input v-model="caption" type="text" placeholder="例: 明るく、少し早口で" /></label>
      <div class="row"><UiButton icon="person_add" :disabled="disabled || busy" @click="create">{{ busy ? '声を作っています…' : 'この声を作る' }}</UiButton></div>
    </template>
    <p v-if="error" class="error">{{ error }}</p>
  </div>
</template>

<style scoped>
.sample-form{display:flex;flex-direction:column;gap:10px}.sample-form .muted,.sample-form .error{margin:0}
.row{display:flex;gap:8px;flex-wrap:wrap;align-items:center}.picked{font-size:13px}
.field{display:flex;flex-direction:column;gap:5px;font-size:12px;color:var(--text-medium)}.field input{padding:9px 10px;border:1px solid var(--field);border-radius:4px;font-size:14px}
</style>
