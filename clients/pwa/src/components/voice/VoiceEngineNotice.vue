<script setup lang="ts">
import { computed } from "vue";
import {
	describeEngine,
	type EngineStatus,
	gb,
	loadingPercent,
} from "@/voice/engineStatus";
import AppIcon from "../AppIcon.vue";
import UiButton from "../UiButton.vue";

// 読み上げの準備状況の案内（Irodori-TTS の声で読めないときに出す）。音声モデルの準備・読み込み直しは親が行う。
const props = defineProps<{ status: EngineStatus }>();
const emit = defineEmits<{ prepare: [] }>();

const icon = computed(() => {
	switch (props.status.kind) {
		case "checking":
		case "loading":
			return "downloading";
		case "needs-download":
			return "download";
		default:
			return "info";
	}
});
</script>

<template>
  <section class="notice surface" :class="status.kind" role="status">
    <AppIcon :name="icon" />
    <div class="notice-body">
      <p>{{ describeEngine(status) }}</p>
      <template v-if="status.kind === 'loading'">
        <div class="progress" :style="{ '--p': `${loadingPercent(status)}%` }"></div>
        <p class="muted">{{ gb(status.loaded) }} / {{ gb(status.total) }} GB（2 回目からは端末に保存したものを読み込みます）</p>
      </template>
      <div v-if="status.kind === 'needs-download'" class="row"><UiButton icon="download" @click="emit('prepare')">音声モデルを準備する</UiButton></div>
      <div v-if="status.kind === 'browser' && status.retry" class="row"><UiButton variant="secondary" icon="refresh" @click="emit('prepare')">もう一度読み込む</UiButton></div>
    </div>
  </section>
</template>

<style scoped>
.notice { display: flex; gap: 12px; padding: 14px 16px; align-items: flex-start; }
.notice>.app-icon { color: var(--primary); margin-top: 2px; }
.notice.browser>.app-icon, .notice.none>.app-icon { color: var(--text-medium); }
.notice-body { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 10px; }
.notice-body p { margin: 0; font-size: 14px; }
.notice-body .muted { font-size: 12px; }
.row { display: flex; gap: 8px; flex-wrap: wrap; align-items: center; }
.progress { height: 4px; background: var(--line); border-radius: 2px; overflow: hidden; position: relative; }
.progress::after { content: ''; position: absolute; inset: 0 auto 0 0; width: var(--p); background: var(--primary); transition: width .2s ease; }
</style>
