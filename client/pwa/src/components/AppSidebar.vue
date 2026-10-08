<script setup lang="ts">
import { onBeforeUnmount, onMounted, watch } from 'vue'
import { useRoute } from 'vue-router'
import AppIcon from './AppIcon.vue'
import { agents, selectAgent, selectedAgentId } from '../api/agents'
import { logout, type SessionUser } from '../api/auth'
import { buildLoginUrl } from '../api/loginRedirect'

// 管理画面（HomeSidebar）と同じ導線: PC は常設、スマホは ☰ で開くドロワー。
const open = defineModel<boolean>('open', { default: false })
const props = defineProps<{ user: SessionUser | null }>()

const items = [
  { to: '/', icon: 'home', label: 'ホーム' },
  { to: '/chat', icon: 'chat', label: 'チャット' },
  { to: '/todo', icon: 'check_circle', label: 'タスク' },
  { to: '/calendar', icon: 'calendar_month', label: '予定' },
  { to: '/finance', icon: 'account_balance_wallet', label: '家計' },
  { to: '/notes', icon: 'description', label: '共有ノート' },
  { to: '/settings', icon: 'settings', label: '設定' },
]

// 画面遷移したらドロワーを閉じる。
const route = useRoute()
watch(() => route.fullPath, () => { open.value = false })

// 一覧は AppShell が取得する。切り替え先が 1 つ以下なら選択欄は出さない。
function handleAgentChange(event: Event) {
  selectAgent((event.target as HTMLSelectElement).value)
  open.value = false
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

function handleKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') open.value = false
}
onMounted(() => window.addEventListener('keydown', handleKeydown))
onBeforeUnmount(() => window.removeEventListener('keydown', handleKeydown))
</script>

<template>
  <aside class="sidebar" :class="{ open }">
    <RouterLink class="sidebar-brand" to="/">
      <AppIcon name="smart_toy" />
      <span>Agent Desk</span>
    </RouterLink>

    <label v-if="agents.length > 1" class="agent-select">
      <span>エージェント</span>
      <select :value="selectedAgentId" @change="handleAgentChange">
        <option v-for="agent in agents" :key="agent.id" :value="agent.id">{{ agent.name }}</option>
      </select>
    </label>

    <nav class="sidebar-menu">
      <RouterLink v-for="item in items" :key="item.to" :to="item.to" class="menu-item" exact-active-class="active">
        <AppIcon :name="item.icon" />
        <span>{{ item.label }}</span>
      </RouterLink>
    </nav>

    <div class="user-panel">
      <div class="user-info" :title="props.user?.username">
        <AppIcon name="person" />
        <span>{{ props.user?.username ?? '' }}</span>
      </div>
      <div class="user-actions">
        <a v-if="props.user?.role === 'admin'" class="icon-button" href="/admin/" title="管理画面" aria-label="管理画面を開く">
          <AppIcon name="admin_panel_settings" />
        </a>
        <button class="icon-button" type="button" title="ログアウト" aria-label="ログアウト" @click="handleLogout">
          <AppIcon name="logout" />
        </button>
      </div>
    </div>
  </aside>
  <button v-if="open" class="sidebar-backdrop" type="button" aria-label="メニューを閉じる" @click="open = false"></button>
</template>

<style scoped>
.sidebar { width: 240px; flex-shrink: 0; position: sticky; top: 0; height: 100dvh; overflow-y: auto; display: flex; flex-direction: column; padding: 20px 14px; background: var(--surface-1); border-right: 1px solid var(--border-divider); }
.sidebar-brand { display: flex; align-items: center; gap: 8px; margin: 0 6px 20px; font-weight: 800; letter-spacing: .08em; text-transform: uppercase; color: var(--text-high); }
.sidebar-brand .material-symbols-outlined { font-size: 22px; color: var(--primary); }
.agent-select { display: grid; gap: 6px; margin: 0 6px 16px; font-size: 12px; color: var(--text-medium); }
.agent-select select { width: 100%; padding: 8px; border: 1px solid var(--field); border-radius: 4px; background: var(--surface-2); color: var(--text-high); }
.sidebar-menu { display: flex; flex-direction: column; gap: 4px; flex-grow: 1; }
.menu-item { display: flex; align-items: center; gap: 12px; padding: 11px 14px; border-radius: 4px; font-size: 14px; font-weight: 500; color: var(--text-medium); }
.menu-item:hover { color: var(--text-high); background: rgba(255,255,255,.04); }
.menu-item.active { color: var(--primary); background: var(--primary-soft); font-weight: 600; }
.user-panel { display: flex; align-items: center; justify-content: space-between; gap: 8px; margin-top: 12px; padding-top: 12px; border-top: 1px solid var(--border-divider); }
.user-info { display: flex; align-items: center; gap: 8px; min-width: 0; font-size: 13px; font-weight: 600; color: var(--text-medium); }
.user-info span:last-child { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.user-actions { display: flex; gap: 4px; flex-shrink: 0; }
.user-actions .icon-button { margin-left: 0; padding: 6px; }
.sidebar-backdrop { display: none; }
@media (max-width: 768px) {
  .sidebar { position: fixed; left: 0; bottom: 0; width: min(280px, 82vw); z-index: 30; transform: translateX(-100%); transition: transform .25s ease; }
  .sidebar.open { transform: translateX(0); box-shadow: 8px 0 32px rgba(0,0,0,.55); }
  .sidebar-backdrop { display: block; position: fixed; inset: 0; z-index: 20; border: 0; padding: 0; background: rgba(0,0,0,.5); }
}
</style>
