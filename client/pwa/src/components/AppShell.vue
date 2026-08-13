<script setup lang="ts">
import { ref } from 'vue'
import type { SessionUser } from '../api/auth'
import AppHeader from './AppHeader.vue'
import ClientNavigationDrawer from './ClientNavigationDrawer.vue'

defineProps<{ user: SessionUser | null }>()
const menuOpen = ref(false)
</script>

<template>
  <AppHeader :user="user" @open-menu="menuOpen = true" />
  <main class="app-main"><slot /></main>
  <ClientNavigationDrawer :open="menuOpen" :user="user" @close="menuOpen = false" />
</template>
