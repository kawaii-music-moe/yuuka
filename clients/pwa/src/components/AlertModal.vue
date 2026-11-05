<script setup lang="ts">
import UiButton from "./UiButton.vue";
import UiDialog from "./UiDialog.vue";

// 入力不足や処理結果を知らせるモーダル。`message` が空でなければ表示し、OK・背景・Esc で閉じる。
const message = defineModel<string>({ default: "" });
const props = withDefaults(defineProps<{ title?: string }>(), {
	title: "入力内容を確認してください",
});

function close() {
	message.value = "";
}
</script>

<template>
  <UiDialog :open="Boolean(message)" @close="close">
    <div class="alert-body" role="alertdialog" aria-labelledby="alert-title" aria-describedby="alert-message">
      <h2 id="alert-title">{{ props.title }}</h2>
      <p id="alert-message">{{ message }}</p>
      <div class="alert-actions"><UiButton autofocus @click="close">OK</UiButton></div>
    </div>
  </UiDialog>
</template>

<style scoped>
.alert-body { padding: 20px; }
.alert-body h2 { margin: 0 0 10px; font-size: 16px; }
.alert-body p { margin: 0; font-size: 14px; line-height: 1.6; color: var(--text-medium); white-space: pre-line; }
.alert-actions { display: flex; justify-content: flex-end; margin-top: 20px; }
.alert-actions .ui-button { min-width: 88px; }
</style>
