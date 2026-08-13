<script setup lang="ts">
import type { ChatReference } from '@/api'
import AppIcon from './AppIcon.vue'
defineProps<{ reference: ChatReference }>()
const icons: Record<ChatReference['type'], string> = { todo: 'check_circle', calendar: 'calendar_month', finance: 'account_balance_wallet', note: 'description' }
</script>
<template><RouterLink :to="{ path: reference.href, query: { from: 'chat' } }" class="reference-card"><AppIcon :name="icons[reference.type]" /><span><small>{{ reference.meta }}</small><strong>{{ reference.title }}</strong><em>{{ reference.description }}</em></span><AppIcon name="chevron_right" /></RouterLink></template>
<style scoped>.reference-card{display:flex;align-items:center;gap:9px;border:1px solid var(--primary-border);background:#f8faff;padding:9px 10px;border-radius:2px;margin-top:8px;color:#1d2939}.reference-card>span{display:grid;gap:2px;min-width:0;flex:1}.reference-card small{color:#667085;font-size:10px}.reference-card strong{font-size:13px;font-weight:600}.reference-card em{font-size:11px;color:#667085;font-style:normal;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.reference-card>.material-symbols-outlined:first-child{color:var(--primary)}.reference-card>.material-symbols-outlined:last-child{color:#98a2b3}</style>
