<script setup lang="ts">
import { nextTick, ref, watch } from 'vue'
import { agentGateway, type CalendarEvent } from '@/api'
import { ApiError } from '@/api/http'
import AlertModal from '../AlertModal.vue'
import FormField from '../FormField.vue'
import UiButton from '../UiButton.vue'

// 予定の追加フォーム。`date` が入ったら開く。`YYYY-MM-DD` ならその日の 09:00〜10:00、
// `YYYY-MM-DDTHH:MM` ならその時刻から 1 時間を初期値にする。
const date = defineModel<string>('date', { default: '' })
const emit = defineEmits<{ created: [event: CalendarEvent] }>()

const dialog = ref<HTMLDialogElement>()
const form = ref({ title: '', startsAt: '', endsAt: '', description: '' })
const saving = ref(false); const saveError = ref(''); const validationError = ref('')

watch(date, async (value) => {
  await nextTick()
  if (value && !dialog.value?.open) {
    form.value = { title: '', startsAt: value.includes('T') ? value : `${value}T09:00`, endsAt: '', description: '' }
    handleStartChange()
    saveError.value = ''
    dialog.value?.showModal()
  } else if (!value && dialog.value?.open) dialog.value.close()
})

function close() { date.value = '' }
function handleBackdropClick(event: MouseEvent) { if (event.target === dialog.value) close() }

// 開始時刻を動かしたら、終了が開始より前にならないよう 1 時間後へ寄せる。
function handleStartChange() {
  const { startsAt, endsAt } = form.value
  if (!startsAt || (endsAt && endsAt > startsAt)) return
  const end = new Date(startsAt); end.setHours(end.getHours() + 1)
  const pad = (n: number) => String(n).padStart(2, '0')
  form.value.endsAt = `${end.getFullYear()}-${pad(end.getMonth() + 1)}-${pad(end.getDate())}T${pad(end.getHours())}:${pad(end.getMinutes())}`
}

async function save() {
  const { title, startsAt, endsAt, description } = form.value
  const missing = [
    !title.trim() && 'タイトルを入力してください。',
    !startsAt && '開始日時を入力してください。',
    startsAt && endsAt && endsAt < startsAt && '終了日時は開始日時より後にしてください。',
  ].filter(Boolean)
  if (missing.length) { validationError.value = missing.join('\n'); return }
  saving.value = true; saveError.value = ''
  try {
    const created = await agentGateway.createCalendarEvent({ title: title.trim(), startsAt, endsAt: endsAt || undefined, description: description.trim() || undefined })
    emit('created', created)
    close()
  } catch (cause) {
    saveError.value = cause instanceof ApiError && cause.status === 400 ? '入力内容を確認してください。' : '予定を追加できませんでした。もう一度お試しください。'
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <dialog ref="dialog" class="event-modal" @close="close" @click="handleBackdropClick">
    <form class="event-form" novalidate @submit.prevent="save">
      <h2>予定を追加</h2>
      <FormField label="タイトル"><input v-model="form.title" placeholder="例: 打ち合わせ" /></FormField>
      <div class="time-grid">
        <FormField label="開始"><input v-model="form.startsAt" type="datetime-local" @change="handleStartChange" /></FormField>
        <FormField label="終了"><input v-model="form.endsAt" type="datetime-local" /></FormField>
      </div>
      <FormField label="メモ（任意）"><textarea v-model="form.description" class="memo" /></FormField>
      <p v-if="saveError" class="error save-error">{{ saveError }}</p>
      <div class="actions">
        <UiButton variant="secondary" @click="close">キャンセル</UiButton>
        <UiButton type="submit" :disabled="saving">{{ saving ? '追加中…' : '追加' }}</UiButton>
      </div>
    </form>
  </dialog>
  <AlertModal v-model="validationError" />
</template>

<style scoped>
.event-modal { width: min(480px, calc(100vw - 32px)); padding: 0; border: 1px solid var(--line); border-radius: 6px; background: var(--surface-2); color: var(--text-high); }
.event-modal::backdrop { background: rgba(0,0,0,.6); }
.event-form { padding: 20px; display: grid; gap: 14px; }
.event-form h2 { margin: 0; font-size: 16px; }
.time-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 10px; }
.event-form :deep(textarea.memo) { min-height: 80px; }
.save-error { margin: 0; font-size: 13px; }
.actions { display: flex; justify-content: flex-end; gap: 8px; }
@media (max-width: 480px) { .time-grid { grid-template-columns: 1fr; } }
</style>
