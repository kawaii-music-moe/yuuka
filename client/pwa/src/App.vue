<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { getCurrentUser } from './api/auth'
import AppShell from './components/AppShell.vue'
import LoginScreen from './components/LoginScreen.vue'

const checkingSession = ref(true)
const authenticated = ref(false)

async function checkSession() {
  authenticated.value = Boolean(await getCurrentUser())
  checkingSession.value = false
}

onMounted(checkSession)
</script>

<template>
  <div v-if="checkingSession" class="session-loading">Loading…</div>
  <LoginScreen v-else-if="!authenticated" @authenticated="checkSession" />
  <AppShell v-else><RouterView /></AppShell>
</template>

<style scoped>.session-loading { min-height: 100dvh; display: grid; place-items: center; background: #121212; color: #a1a1aa; }</style>
