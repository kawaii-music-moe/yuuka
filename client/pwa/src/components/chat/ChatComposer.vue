<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'
import AppIcon from '../AppIcon.vue'
import { CHAT_ATTACHMENT_ACCEPT, chatAttachmentError, mediaKind } from '@/utils/chatAttachments'

// Discord 風の入力バー（＋で添付・本文・送信）。本文か添付のどちらかがあれば送信できる。
// 送信が同期的に拒否されたときは、親が `restore` で本文と添付を戻す。
defineProps<{ disabled: boolean }>()
const emit = defineEmits<{ send: [payload: { content: string; files: File[] }] }>()

type Pending = { id: number; file: File; previewUrl?: string }
let nextId = 0

const draft = ref('')
const pending = ref<Pending[]>([])
const error = ref('')
const input = ref<HTMLTextAreaElement>()
const picker = ref<HTMLInputElement>()
const canSend = computed(() => draft.value.trim() !== '' || pending.value.length > 0)

// 入力欄は 1 行から始め、内容に合わせて最大 8 行程度まで伸ばす（それ以上は欄の中でスクロール）。
const MAX_INPUT_HEIGHT = 200
watch(draft, async () => {
  await nextTick()
  const el = input.value
  if (!el) return
  el.style.height = 'auto'
  el.style.height = `${Math.min(el.scrollHeight, MAX_INPUT_HEIGHT)}px`
})

function toPending(file: File): Pending {
  return { id: nextId++, file, previewUrl: mediaKind(file.type) === 'image' ? URL.createObjectURL(file) : undefined }
}
function release(item: Pending) {
  if (item.previewUrl) URL.revokeObjectURL(item.previewUrl)
}

function addFiles(files: File[]) {
  const message = chatAttachmentError(pending.value.map((item) => item.file), files)
  if (message) {
    error.value = message
    return
  }
  error.value = ''
  pending.value.push(...files.map(toPending))
}
function handlePick(event: Event) {
  const target = event.target as HTMLInputElement
  addFiles([...(target.files ?? [])])
  target.value = '' // 同じファイルを選び直しても change が発火するようにする
}
// 画像などを入力欄へ貼り付けたときも添付にする。
function handlePaste(event: ClipboardEvent) {
  const files = [...(event.clipboardData?.files ?? [])]
  if (!files.length) return
  event.preventDefault()
  addFiles(files)
}
function remove(item: Pending) {
  release(item)
  pending.value = pending.value.filter((other) => other !== item)
  error.value = ''
}

function submit() {
  if (!canSend.value) return
  const payload = { content: draft.value.trim(), files: pending.value.map((item) => item.file) }
  pending.value.forEach(release)
  draft.value = ''
  pending.value = []
  error.value = ''
  emit('send', payload)
}
function handleKeydown(event: KeyboardEvent) {
  if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
    event.preventDefault()
    submit()
  }
}

defineExpose({
  restore(content: string, files: File[]) {
    draft.value = content
    pending.value = files.map(toPending)
  },
})
onBeforeUnmount(() => pending.value.forEach(release))
</script>

<template>
  <form class="composer" @submit.prevent="submit">
    <ul v-if="pending.length" class="pending">
      <li v-for="item in pending" :key="item.id" :title="item.file.name">
        <img v-if="item.previewUrl" :src="item.previewUrl" :alt="item.file.name" />
        <span v-else class="file"><AppIcon :name="{ audio: 'music_note', video: 'movie', image: 'image', file: 'description' }[mediaKind(item.file.type)]" /><small>{{ item.file.name }}</small></span>
        <button type="button" class="remove" :aria-label="`${item.file.name} を外す`" @click="remove(item)"><AppIcon name="close" /></button>
      </li>
    </ul>
    <p v-if="error" class="composer-error" role="alert">{{ error }}</p>
    <div class="bar">
      <button type="button" class="icon-button attach" :disabled="disabled" aria-label="ファイルを添付" title="ファイルを添付" @click="picker?.click()"><AppIcon name="add_circle" /></button>
      <input ref="picker" class="file-input" type="file" multiple :accept="CHAT_ATTACHMENT_ACCEPT" @change="handlePick" />
      <textarea ref="input" v-model="draft" rows="1" :disabled="disabled" placeholder="メッセージを入力" aria-label="メッセージ" @keydown="handleKeydown" @paste="handlePaste" />
      <button v-if="canSend" class="icon-button send" type="submit" :disabled="disabled" aria-label="送信" title="送信"><AppIcon name="send" /></button>
    </div>
  </form>
</template>

<style scoped>
/* Discord 風の角丸の入力バー。添付はバーの上にサムネイルで並べる。 */
.composer { margin-top: 8px; border-radius: 8px; background: #2a2a2a; }
.bar { display: flex; align-items: flex-end; gap: 2px; padding: 4px 6px; }
.bar textarea { flex: 1; min-width: 0; max-height: 200px; padding: 11px 6px; border: 0; resize: none; outline: 0; background: transparent; color: var(--text-high); line-height: 1.5; overflow-y: auto; }
.bar textarea::placeholder { color: var(--text-medium); }
.icon-button { flex: none; width: 40px; height: 40px; margin-bottom: 2px; display: grid; place-items: center; border: 0; border-radius: 6px; background: transparent; }
.icon-button:hover:not(:disabled) { background: rgba(255,255,255,.06); }
.icon-button:disabled { opacity: .5; cursor: default; }
.attach { color: var(--text-medium); }
.attach:hover:not(:disabled) { color: var(--text-high); }
.send { color: var(--primary); }
.file-input { display: none; }
.pending { display: flex; gap: 8px; overflow-x: auto; margin: 0; padding: 10px 10px 4px; list-style: none; }
.pending li { position: relative; flex: none; width: 88px; height: 88px; border-radius: 6px; overflow: hidden; background: #1d1d1d; border: 1px solid var(--line); }
.pending img { width: 100%; height: 100%; object-fit: cover; }
.pending .file { height: 100%; display: grid; place-items: center; align-content: center; gap: 4px; padding: 6px; color: var(--text-medium); }
.pending .file small { max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 10px; }
.remove { position: absolute; top: 3px; right: 3px; width: 22px; height: 22px; display: grid; place-items: center; border: 0; border-radius: 50%; background: rgba(0,0,0,.7); color: #fff; }
.remove .material-symbols-outlined { font-size: 16px; }
.composer-error { margin: 0; padding: 8px 12px 0; font-size: 12px; color: #cf6679; }
</style>
