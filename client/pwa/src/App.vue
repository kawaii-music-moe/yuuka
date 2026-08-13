<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { getCurrentUser, type SessionUser } from './api/auth'
import AppShell from './components/AppShell.vue'

const checkingSession = ref(true)
const authenticated = ref(false)
const user = ref<SessionUser | null>(null)

async function checkSession() {
  user.value = await getCurrentUser()
  authenticated.value = Boolean(user.value)
  if (!authenticated.value) {
    const returnTo = `${window.location.pathname}${window.location.search}${window.location.hash}`
    window.location.replace(`/login?returnTo=${encodeURIComponent(returnTo)}`)
    return
  }
  checkingSession.value = false
}

onMounted(checkSession)
</script>

<template>
  <div v-if="checkingSession" class="session-loading">Loading…</div>
  <AppShell v-else-if="authenticated" :user="user"><RouterView /></AppShell>
</template>

<style scoped>.session-loading { min-height: 100dvh; display: grid; place-items: center; background: #121212; color: #a1a1aa; }</style>
