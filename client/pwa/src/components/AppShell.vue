<script setup lang="ts">
import { ref } from 'vue'
import { selectedAgentId } from '../api/agents'
import type { SessionUser } from '../api/auth'
import AppHeader from './AppHeader.vue'
import AppSidebar from './AppSidebar.vue'

defineProps<{ user: SessionUser | null }>()
const menuOpen = ref(false)
</script>

<template>
  <div class="app-layout">
    <AppSidebar v-model:open="menuOpen" :user="user" />
    <div class="app-column">
      <AppHeader @open-menu="menuOpen = true" />
      <!-- エージェントを切り替えたら画面を作り直し、各ページが新しいエージェントのデータを読み直す。 -->
      <main :key="selectedAgentId" class="app-main"><slot /></main>
    </div>
  </div>
</template>

<style scoped>
.app-layout { display: flex; min-height: 100dvh; }
.app-column { flex: 1; min-width: 0; }
</style>
