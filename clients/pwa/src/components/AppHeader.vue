<script setup lang="ts">
import { computed } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import AppIcon from './AppIcon.vue'

const route = useRoute()
const router = useRouter()
const title = computed(() => route.meta.title ?? 'Agent Desk')
const fromChat = computed(() => route.query.from === 'chat')
const emit = defineEmits<{ openMenu: [] }>()
</script>

<template>
  <header class="app-header">
    <button class="menu-toggle" type="button" aria-label="メニューを開く" @click="emit('openMenu')">
      <AppIcon name="menu" />
    </button>
    <h1>{{ title }}</h1>
    <button v-if="fromChat" class="back-button" type="button" @click="router.push('/chat')">
      <AppIcon name="arrow_back" />
      <span>チャットに戻る</span>
    </button>
  </header>
</template>

<style scoped>
.menu-toggle { display: none; padding: 2px; border: 0; background: transparent; color: var(--text-high); }
.menu-toggle .material-symbols-outlined { font-size: 26px; }
@media (max-width: 768px) { .menu-toggle { display: inline-flex; } }
</style>
