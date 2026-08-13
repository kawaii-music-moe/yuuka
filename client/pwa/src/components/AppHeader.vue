<script setup lang="ts">
import { computed } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import AppIcon from './AppIcon.vue'
const route = useRoute()
const router = useRouter()
const title = computed(() => route.meta.title ?? 'Agent Desk')
const fromChat = computed(() => route.query.from === 'chat')
</script>
<template>
  <header class="app-header"><RouterLink class="brand" to="/"><AppIcon name="smart_toy" /> <span>Agent Desk</span></RouterLink><h1>{{ title }}</h1><button v-if="fromChat" class="back-button" type="button" @click="router.push('/chat')"><AppIcon name="arrow_back" /> <span>チャットに戻る</span></button><RouterLink v-else class="icon-button" to="/settings" aria-label="設定"><AppIcon name="settings" /></RouterLink></header>
</template>
