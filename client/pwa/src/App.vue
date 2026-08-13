<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useRoute } from 'vue-router'
import { getCurrentUser, type SessionUser } from './api/auth'
import AppShell from './components/AppShell.vue'

const checkingSession = ref(true)
const authenticated = ref(false)
const user = ref<SessionUser | null>(null)
const route = useRoute()
const isPublicRoute = computed(() => route.meta.public === true)

async function checkSession() {
  if (isPublicRoute.value) {
    checkingSession.value = false
    return
  }
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
  <RouterView v-else-if="isPublicRoute" />
  <AppShell v-else-if="authenticated" :user="user"><RouterView /></AppShell>
</template>

<style scoped>.session-loading { min-height: 100dvh; display: grid; place-items: center; background: #121212; color: #a1a1aa; }</style>
