<script setup lang="ts">
import { selectedAgentId } from '../api/agents'
import type { SessionUser } from '../api/auth'
import AppHeader from './AppHeader.vue'
import BottomNavigation from './BottomNavigation.vue'

defineProps<{ user: SessionUser | null }>()
</script>

<template>
  <AppHeader :user="user" />
  <!-- エージェントを切り替えたら画面を作り直し、各ページが新しいエージェントのデータを読み直す。 -->
  <main :key="selectedAgentId" class="app-main"><slot /></main>
  <BottomNavigation />
</template>
