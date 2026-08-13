<script setup lang="ts">
import type { SessionUser } from '../api/auth'
import AppHeader from './AppHeader.vue'
import BottomNavigation from './BottomNavigation.vue'

defineProps<{ user: SessionUser | null }>()
</script>

<template>
  <AppHeader :user="user" />
  <main class="app-main"><slot /></main>
  <BottomNavigation />
</template>
