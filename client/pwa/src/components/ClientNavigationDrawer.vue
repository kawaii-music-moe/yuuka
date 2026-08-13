<script setup lang="ts">
import { useRouter } from 'vue-router'
import { logout, type SessionUser } from '../api/auth'
import AppIcon from './AppIcon.vue'

const props = defineProps<{ open: boolean; user: SessionUser | null }>()
const emit = defineEmits<{ close: [] }>()
const router = useRouter()

const items = [
  { to: '/', icon: 'home', label: 'ホーム' },
  { to: '/chat', icon: 'chat', label: 'チャット' },
  { to: '/todo', icon: 'check_circle', label: 'タスク' },
  { to: '/calendar', icon: 'calendar_month', label: '予定' },
  { to: '/finance', icon: 'account_balance_wallet', label: '家計' },
  { to: '/notes', icon: 'note', label: '共有ノート' },
  { to: '/settings', icon: 'settings', label: '設定' },
]

function close() { emit('close') }
async function signOut() {
  await logout()
  close()
  window.location.assign('/login')
}
</script>

<template>
  <Teleport to="body">
    <Transition name="drawer-backdrop">
      <button v-if="open" class="drawer-backdrop" type="button" aria-label="メニューを閉じる" @click="close" />
    </Transition>
    <Transition name="drawer-panel">
      <aside v-if="open" class="navigation-drawer" aria-label="メインメニュー">
        <div class="drawer-brand">
          <AppIcon name="smart_toy" />
          <span>Agent Desk</span>
          <button type="button" class="drawer-close" aria-label="メニューを閉じる" @click="close"><AppIcon name="close" /></button>
        </div>
        <nav class="drawer-nav">
          <RouterLink v-for="item in items" :key="item.to" :to="item.to" @click="close">
            <AppIcon :name="item.icon" /><span>{{ item.label }}</span>
          </RouterLink>
        </nav>
        <div class="drawer-footer">
          <a v-if="props.user?.role === 'admin'" class="drawer-admin" href="/admin/" @click="close">
            <AppIcon name="admin_panel_settings" /><span>管理画面</span>
          </a>
          <div class="drawer-user"><AppIcon name="account_circle" /><span>{{ props.user?.username ?? 'ユーザー' }}</span></div>
          <button type="button" class="drawer-logout" @click="signOut"><AppIcon name="logout" /><span>ログアウト</span></button>
        </div>
      </aside>
    </Transition>
  </Teleport>
</template>
