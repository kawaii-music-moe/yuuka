<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useRoute } from 'vue-router'
import { getCurrentUser, type SessionUser } from './api/auth'
import { agentGateway } from './api'
import AppShell from './components/AppShell.vue'
import { selectClientBot, selectClientBotById, selectedBot, setAvailableBots } from './stores/botSelection'

const checkingSession = ref(true)
const authenticated = ref(false)
const user = ref<SessionUser | null>(null)
const route = useRoute()
const isPublicRoute = computed(() => route.meta.public === true)

function loginUrl(returnTo: string): string {
  const target = new URL('/login', window.location.origin)
  if (target.port === '5173') target.port = '5174'
  target.searchParams.set('returnTo', returnTo)
  return target.toString()
}

async function checkSession() {
  if (isPublicRoute.value) {
    checkingSession.value = false
    return
  }
  user.value = await getCurrentUser()
  authenticated.value = Boolean(user.value)
  if (!authenticated.value) {
    const returnTo = window.location.port === '5173'
      ? window.location.href
      : `${window.location.pathname}${window.location.search}${window.location.hash}`
    window.location.replace(loginUrl(returnTo))
    return
  }
  try {
    const bots = await agentGateway.listBots()
    setAvailableBots(bots)
    const requestedBotId = typeof route.query.botId === 'string' ? route.query.botId : undefined
    selectClientBotById(requestedBotId)
    if (!selectedBot.value && bots[0]) selectClientBot(bots[0])
  } catch {
    // Bot 一覧が一時的に取得できなくても、既存の Client 機能は表示を継続する。
  }
  checkingSession.value = false
}

onMounted(checkSession)
watch(() => route.query.botId, (botId) => selectClientBotById(typeof botId === 'string' ? botId : undefined))
</script>

<template>
  <div v-if="checkingSession" class="session-loading">Loading…</div>
  <RouterView v-else-if="isPublicRoute" />
  <AppShell v-else-if="authenticated" :user="user"><RouterView /></AppShell>
</template>

<style scoped>.session-loading { min-height: 100dvh; display: grid; place-items: center; background: #121212; color: #a1a1aa; }</style>
