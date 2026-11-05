<script setup lang="ts">
import { nextTick, ref, watch } from "vue";

// `<dialog>` のモーダル。`open` に合わせて開け閉めする。Esc・背景のクリックでは `close` を出すだけで、
// 閉じるかどうかは親が `open` で決める（閉じるときの後始末を親が持てるように）。
const props = withDefaults(defineProps<{ open: boolean; width?: number }>(), {
	width: 400,
});
const emit = defineEmits<{ close: [] }>();
const dialog = ref<HTMLDialogElement>();

watch(
	() => props.open,
	async (open) => {
		await nextTick();
		if (open && !dialog.value?.open) dialog.value?.showModal();
		else if (!open && dialog.value?.open) dialog.value.close();
	},
	{ immediate: true },
);

// Esc で閉じたとき（ブラウザが閉じる）。親が閉じたときにも発火するので、開いている間だけ知らせる。
function handleClose() {
	if (props.open) emit("close");
}
function handleBackdropClick(event: MouseEvent) {
	if (event.target === dialog.value) emit("close");
}
</script>

<template>
  <dialog ref="dialog" class="ui-dialog" :style="{ width: `min(${width}px, calc(100vw - 32px))` }" @close="handleClose" @click="handleBackdropClick">
    <slot />
  </dialog>
</template>

<style scoped>
.ui-dialog { padding: 0; border: 1px solid var(--line); border-radius: 6px; background: var(--surface-2); color: var(--text-high); }
.ui-dialog::backdrop { background: rgba(0,0,0,.6); }
</style>
