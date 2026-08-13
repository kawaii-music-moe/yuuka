<script setup lang="ts">
import { computed } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import AppIcon from './AppIcon.vue'
import type { SessionUser } from '../api/auth'

const route = useRoute()
const router = useRouter()
const title = computed(() => route.meta.title ?? 'Agent Desk')
const fromChat = computed(() => route.query.from === 'chat')
defineProps<{ user: SessionUser | null }>()
defineEmits<{ openMenu: [] }>()
</script>

<template>
  <header class="app-header">
    <button class="menu-trigger" type="button" aria-label="メニューを開く" @click="$emit('openMenu')"><AppIcon name="menu" /></button>
    <RouterLink class="brand" to="/">
      <AppIcon name="smart_toy" />
      <span>Agent Desk</span>
    </RouterLink>
    <h1>{{ title }}</h1>
    <div class="header-actions">
      <button v-if="fromChat" class="back-button" type="button" @click="router.push('/chat')">
        <AppIcon name="arrow_back" />
        <span>Back to chat</span>
      </button>
    </div>
  </header>
</template>

<style scoped>
.header-actions { display: flex; gap: 4px; align-items: center; }
</style>
