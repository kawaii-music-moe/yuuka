<script setup lang="ts">
import { ref } from 'vue'
import { agentGateway, type ReceiptDraft } from '@/api'
import { ApiError } from '@/api/http'
import UiButton from '../UiButton.vue'
import { prepareReceiptImage, RECEIPT_MIME_TYPES } from '@/utils/receiptImage'

// レシート画像を選んで読み取り、下書きを親へ渡す（登録はしない）。`filled` の間は「入力しました」の案内を出す。
defineProps<{ filled: boolean }>()
const emit = defineEmits<{ scanned: [draft: ReceiptDraft] }>()

const scanning = ref(false)
const error = ref('')
const input = ref<HTMLInputElement>()

async function handleChange(event: Event) {
  const target = event.target as HTMLInputElement
  const file = target.files?.[0]
  target.value = '' // 同じ画像を選び直しても change が発火するようにする
  if (!file) return
  // 拡張子だけで MIME が空になる端末もあるため、空なら送ってサーバーの判定に任せる。
  if (file.type && !RECEIPT_MIME_TYPES.includes(file.type.toLowerCase())) {
    error.value = 'PNG・JPEG・WebP・HEIC・GIF の画像を選んでください。'
    return
  }
  scanning.value = true
  error.value = ''
  try {
    emit('scanned', await agentGateway.scanReceipt(await prepareReceiptImage(file)))
  } catch (cause) {
    error.value = cause instanceof ApiError && cause.serverMessage ? cause.serverMessage : 'レシートを読み取れませんでした。もう一度お試しください。'
  } finally {
    scanning.value = false
  }
}
</script>

<template>
  <section class="receipt">
    <div>
      <h3>レシートから記録</h3>
      <p>レシートの写真を選ぶと、読み取った内容を下の入力欄に入れます。</p>
      <p v-if="error" class="scan-error">{{ error }}</p>
      <p v-else-if="filled" class="scan-filled">読み取った内容を下の欄に入力しました。確認・修正してから「登録」を押してください。</p>
    </div>
    <UiButton variant="secondary" icon="receipt_long" :disabled="scanning" @click="input?.click()">{{ scanning ? '読み取り中…' : 'レシート画像を選択' }}</UiButton>
    <input ref="input" class="file-input" type="file" accept="image/*" @change="handleChange" />
  </section>
</template>

<style scoped>
.receipt { padding: 16px 0; border-top: 1px solid var(--line); display: flex; align-items: center; justify-content: space-between; gap: 16px; }
.receipt h3 { margin: 0; font-size: 14px; }
.receipt p { margin: 4px 0 0; font-size: 12px; color: var(--text-medium); }
.receipt .scan-error { color: #cf6679; }
.receipt .scan-filled { color: var(--primary); }
.file-input { display: none; }
@media (max-width: 640px) { .receipt { flex-direction: column; align-items: stretch; } }
</style>
