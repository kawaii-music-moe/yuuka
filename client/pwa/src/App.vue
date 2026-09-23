<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { getCurrentUser, type SessionUser } from './api/auth'
import AppShell from './components/AppShell.vue'
import UiButton from './components/UiButton.vue'

const checkingSession = ref(true)
const authenticated = ref(false)
const sessionError = ref(false)
const user = ref<SessionUser | null>(null)

async function checkSession() {
  checkingSession.value = true
  sessionError.value = false
  try {
    user.value = await getCurrentUser()
  } catch {
    // `getCurrentUser` は 401 のみ null に丸める。404/500/ネットワークエラー等はここまで
    // 例外が伝播するため、捕まえないと `checkingSession` が false にならず「Loading…」のまま
    // 固まってしまう（issue #47）。
    sessionError.value = true
    checkingSession.value = false
    return
  }
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
  <div v-else-if="sessionError" class="session-loading session-error">
    <p>セッションを確認できませんでした。</p>
    <UiButton variant="secondary" @click="checkSession">再試行</UiButton>
  </div>
  <AppShell v-else-if="authenticated" :user="user"><RouterView /></AppShell>
</template>

<style scoped>
.session-loading { min-height: 100dvh; display: grid; place-items: center; background: #121212; color: #a1a1aa; }
.session-error { gap: 16px; text-align: center; }
.session-error p { margin: 0; }
</style>
