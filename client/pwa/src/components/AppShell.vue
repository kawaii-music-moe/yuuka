<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from 'vue'
import { agents, refreshAgents, selectedAgentId } from '../api/agents'
import type { SessionUser } from '../api/auth'
import AppHeader from './AppHeader.vue'
import AppSidebar from './AppSidebar.vue'
import PageState from './PageState.vue'
import UiButton from './UiButton.vue'

defineProps<{ user: SessionUser | null }>()
const menuOpen = ref(false)

// PWA の画面はすべて選択中のエージェント（自分の Bot・共有された Bot）のデータを扱う。
// 一覧を取るまでは、前回選んでいたエージェントがあればそのまま表示し、無ければ待つ。
// 管理画面で Bot を作った・共有されたあとに戻ってきたときも候補に出るよう、画面に戻るたびに取り直す。
const agentsLoaded = ref(false)
const agentsError = ref('')
async function loadAgents() {
  try {
    await refreshAgents()
    agentsError.value = ''
  } catch {
    agentsError.value = 'エージェントの一覧を取得できませんでした。'
  } finally {
    agentsLoaded.value = true
  }
}
function handleVisibilityChange() {
  if (document.visibilityState === 'visible') loadAgents()
}
onMounted(() => {
  loadAgents()
  document.addEventListener('visibilitychange', handleVisibilityChange)
})
onBeforeUnmount(() => document.removeEventListener('visibilitychange', handleVisibilityChange))
</script>

<template>
  <div class="app-layout">
    <AppSidebar v-model:open="menuOpen" :user="user" />
    <div class="app-column">
      <AppHeader @open-menu="menuOpen = true" />
      <!-- エージェントを切り替えたら画面を作り直し、各ページが新しいエージェントのデータを読み直す。 -->
      <main v-if="selectedAgentId" :key="selectedAgentId" class="app-main"><slot /></main>
      <main v-else class="app-main">
        <PageState :loading="!agentsLoaded" :error="agentsError" />
        <div v-if="agentsError" class="no-agent">
          <UiButton variant="secondary" @click="loadAgents">再試行</UiButton>
        </div>
        <div v-else-if="agentsLoaded && agents.length === 0" class="no-agent">
          <p>使えるエージェントがありません。管理画面で Bot を作成するか、共有してもらってください。</p>
          <!-- 管理画面は PWA のルーター外（別アプリ）なので通常のリンクで開く。 -->
          <a class="admin-link" href="/admin/">管理画面を開く</a>
        </div>
      </main>
    </div>
  </div>
</template>

<style scoped>
.app-layout { display: flex; min-height: 100dvh; }
.app-column { flex: 1; min-width: 0; }
.no-agent { display: grid; justify-items: start; gap: 14px; font-size: 14px; color: var(--text-medium); }
.no-agent p { margin: 0; }
.admin-link { color: var(--primary); text-decoration: underline; }
</style>
