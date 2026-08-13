<script setup lang="ts">
import { ref } from 'vue'
import { login } from '@/api/auth'
import AppIcon from './AppIcon.vue'
import UiButton from './UiButton.vue'

const emit = defineEmits<{ authenticated: [] }>()
const account = ref('')
const password = ref('')
const error = ref('')
const submitting = ref(false)

async function submit() {
  if (!account.value || !password.value) return
  submitting.value = true
  error.value = ''
  try {
    await login(account.value, password.value)
    emit('authenticated')
  } catch {
    error.value = 'Account ID or password is incorrect.'
  } finally {
    submitting.value = false
  }
}
</script>

<template>
  <main class="login-screen">
    <form class="login-panel" @submit.prevent="submit">
      <div class="login-brand"><AppIcon name="calculate" /><span>Yuuka</span></div>
      <p>Agent control panel</p>
      <label>Account ID<input v-model="account" autocomplete="username" /></label>
      <label>Password<input v-model="password" type="password" autocomplete="current-password" /></label>
      <p v-if="error" class="login-error">{{ error }}</p>
      <UiButton type="submit" :disabled="submitting">{{ submitting ? 'Signing in…' : 'Sign in' }}</UiButton>
      <small>Mock credentials: admin / pass</small>
    </form>
  </main>
</template>

<style scoped>
.login-screen { min-height: 100dvh; display: grid; place-items: center; padding: 24px; background: #121212; color: #f4f4f5; }
.login-panel { width: min(100%, 380px); border: 1px solid #3f3f46; background: #18181b; padding: 28px; display: grid; gap: 14px; }
.login-brand { display: flex; align-items: center; gap: 8px; color: #60a5fa; font-size: 24px; font-weight: 700; }.login-panel p { margin: 0; color: #a1a1aa; font-size: 13px; }
.login-panel label { display: grid; gap: 7px; font-size: 12px; color: #d4d4d8; }.login-panel input { min-height: 42px; border: 1px solid #52525b; background: #09090b; color: #f4f4f5; padding: 9px; border-radius: 2px; }
.login-panel small { color: #71717a; font-size: 11px; }.login-error { color: #fca5a5 !important; }
</style>
