<script setup lang="ts">
import { nextTick, ref, watch } from 'vue'
import UiButton from './UiButton.vue'

// 入力不足や処理結果を知らせるモーダル。`message` が空でなければ表示し、OK・背景・Esc で閉じる。
const message = defineModel<string>({ default: '' })
const props = withDefaults(defineProps<{ title?: string }>(), { title: '入力内容を確認してください' })
const dialog = ref<HTMLDialogElement>()

watch(message, async (value) => {
  await nextTick()
  if (value && !dialog.value?.open) dialog.value?.showModal()
  else if (!value && dialog.value?.open) dialog.value.close()
})

function close() { message.value = '' }
function handleBackdropClick(event: MouseEvent) { if (event.target === dialog.value) close() }
</script>

<template>
  <dialog ref="dialog" class="alert-modal" @close="close" @click="handleBackdropClick">
    <div class="alert-body" role="alertdialog" aria-labelledby="alert-title" aria-describedby="alert-message">
      <h2 id="alert-title">{{ props.title }}</h2>
      <p id="alert-message">{{ message }}</p>
      <div class="alert-actions"><UiButton autofocus @click="close">OK</UiButton></div>
    </div>
  </dialog>
</template>

<style scoped>
.alert-modal { width: min(400px, calc(100vw - 32px)); padding: 0; border: 1px solid var(--line); border-radius: 6px; background: var(--surface-2); color: var(--text-high); }
.alert-modal::backdrop { background: rgba(0,0,0,.6); }
.alert-body { padding: 20px; }
.alert-body h2 { margin: 0 0 10px; font-size: 16px; }
.alert-body p { margin: 0; font-size: 14px; line-height: 1.6; color: var(--text-medium); white-space: pre-line; }
.alert-actions { display: flex; justify-content: flex-end; margin-top: 20px; }
.alert-actions .ui-button { min-width: 88px; }
</style>
