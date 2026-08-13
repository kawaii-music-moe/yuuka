<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { getCurrentUser } from './api/auth'
import AppShell from './components/AppShell.vue'

const checkingSession = ref(true)
const authenticated = ref(false)

async function checkSession() {
  authenticated.value = Boolean(await getCurrentUser())
  if (!authenticated.value) {
    const returnTo = `${window.location.pathname}${window.location.search}${window.location.hash}`
    window.location.replace(`/admin/login?returnTo=${encodeURIComponent(returnTo)}`)
    return
  }
  checkingSession.value = false
}

onMounted(checkSession)
</script>

<template>
  <div v-if="checkingSession" class="session-loading">Loading…</div>
  <AppShell v-else-if="authenticated"><RouterView /></AppShell>
</template>

<style scoped>.session-loading { min-height: 100dvh; display: grid; place-items: center; background: #121212; color: #a1a1aa; }</style>
