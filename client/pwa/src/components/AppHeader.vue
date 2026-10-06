<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import AppIcon from './AppIcon.vue'
import { listAgents, selectAgent, selectedAgentId, type Agent } from '../api/agents'
import { logout, type SessionUser } from '../api/auth'
import { buildLoginUrl } from '../api/loginRedirect'

const route = useRoute()
const router = useRouter()
const title = computed(() => route.meta.title ?? 'Agent Desk')
const fromChat = computed(() => route.query.from === 'chat')
defineProps<{ user: SessionUser | null }>()

// 切り替え先が 1 つ（秘書 Bot のみ）なら選択欄は出さない。一覧を取れなくても既定のエージェントで使える。
const agents = ref<Agent[]>([])
onMounted(async () => {
  try {
    agents.value = await listAgents()
  } catch {
    agents.value = []
  }
})

function handleAgentChange(event: Event) {
  selectAgent((event.target as HTMLSelectElement).value)
}

async function handleLogout() {
  try {
    await logout()
  } catch {
    // セッションが既に切れている場合でも、ログイン画面へは戻す。
  } finally {
    // 再ログイン後はこの PWA のホームへ戻す（returnTo 無しだと管理画面の Bot 選択へ着地する）。
    window.location.assign(buildLoginUrl('/'))
  }
}
</script>

<template>
  <header class="app-header">
    <RouterLink class="brand" to="/">
      <AppIcon name="smart_toy" />
      <span>Agent Desk</span>
    </RouterLink>
    <h1>{{ title }}</h1>
    <div class="header-actions">
      <label v-if="agents.length > 1" class="agent-select">
        <AppIcon name="smart_toy" />
        <span class="visually-hidden">エージェント</span>
        <select :value="selectedAgentId" aria-label="エージェントを切り替え" @change="handleAgentChange">
          <option v-for="agent in agents" :key="agent.id" :value="agent.id">{{ agent.name }}</option>
        </select>
      </label>
      <button v-if="fromChat" class="back-button" type="button" @click="router.push('/chat')">
        <AppIcon name="arrow_back" />
        <span>Back to chat</span>
      </button>
      <a v-if="user?.role === 'admin'" class="icon-button" href="/admin/" aria-label="Open administration">
        <AppIcon name="admin_panel_settings" />
      </a>
      <RouterLink class="icon-button" to="/settings" aria-label="Open settings">
        <AppIcon name="settings" />
      </RouterLink>
      <button class="icon-button" type="button" aria-label="Log out" @click="handleLogout">
        <AppIcon name="logout" />
      </button>
    </div>
  </header>
</template>

<style scoped>
.header-actions { display: flex; gap: 4px; align-items: center; min-width: 0; }
.agent-select { display: flex; align-items: center; gap: 4px; margin-right: 8px; min-width: 0; color: var(--primary); }
.agent-select select { min-width: 0; max-width: 180px; padding: 6px 8px; border: 1px solid var(--field); border-radius: 4px; background: var(--surface-2); color: var(--text-high); text-overflow: ellipsis; }
.visually-hidden { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); white-space: nowrap; }
@media (max-width: 640px) { .agent-select .material-symbols-outlined { display: none; } .agent-select select { max-width: 120px; } }
</style>
