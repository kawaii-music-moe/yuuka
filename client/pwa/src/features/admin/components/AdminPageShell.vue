<script setup lang="ts">
import { computed } from 'vue'
import { useRoute } from 'vue-router'
import { AppIcon } from '@/components/ui'
import { selectedBot } from '@/stores/botSelection'

defineProps<{ title: string }>()

const route = useRoute()
const clientHref = computed(() => ({ path: '/', query: selectedBot.value ? { botId: selectedBot.value.id } : route.query }))
</script>

<template>
  <section class="admin-page-shell">
    <header class="admin-page-shell__header">
      <RouterLink class="admin-page-shell__back" :to="clientHref" aria-label="Clientへ戻る">
        <AppIcon name="arrow_back" />
      </RouterLink>
      <h1>{{ title }}</h1>
      <slot name="actions" />
    </header>
    <div class="admin-page-shell__content"><slot /></div>
  </section>
</template>

<style scoped>
.admin-page-shell { min-height:100%; }
.admin-page-shell__header { min-height:58px; display:flex; align-items:center; gap:12px; padding:0 16px; border-bottom:1px solid var(--border-divider); background:var(--surface-1); }
.admin-page-shell__header h1 { min-width:0; margin:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; font-size:16px; font-weight:700; }
.admin-page-shell__back { width:36px; height:36px; display:grid; place-items:center; border-radius:4px; color:var(--primary); }
.admin-page-shell__back:hover { background:var(--primary-soft); }
.admin-page-shell__content { padding:28px 24px 44px; }
@media (max-width:640px) { .admin-page-shell__content { padding:18px 16px 30px; } }
</style>
