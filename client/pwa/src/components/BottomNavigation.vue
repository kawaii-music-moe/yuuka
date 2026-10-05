<script setup lang="ts">
import AppIcon from './AppIcon.vue'
const items = [{ to: '/', icon: 'home', label: 'ホーム' }, { to: '/chat', icon: 'chat', label: 'チャット' }, { to: '/todo', icon: 'check_circle', label: 'タスク' }, { to: '/calendar', icon: 'calendar_month', label: '予定' }, { to: '/finance', icon: 'account_balance_wallet', label: '家計' }]
</script>
<template><nav class="bottom-nav"> <RouterLink v-for="item in items" :key="item.to" :to="item.to"><AppIcon :name="item.icon" /><span>{{ item.label }}</span></RouterLink></nav></template>
