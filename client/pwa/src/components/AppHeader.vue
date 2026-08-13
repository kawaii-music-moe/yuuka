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
</script>

<template>
  <header class="app-header">
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
      <a v-if="user?.role === 'admin'" class="icon-button" href="/admin/" aria-label="Open administration">
        <AppIcon name="admin_panel_settings" />
      </a>
      <RouterLink class="icon-button" to="/settings" aria-label="Open settings">
        <AppIcon name="settings" />
      </RouterLink>
    </div>
  </header>
</template>

<style scoped>
.header-actions { display: flex; gap: 4px; align-items: center; }
</style>
