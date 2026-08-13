<script setup lang="ts">
defineProps<{ title: string; description?: string }>()
</script>
<template><div class="page-title"><div><h2>{{ title }}</h2><p v-if="description">{{ description }}</p></div><div v-if="$slots.actions" class="page-title__actions"><slot name="actions" /></div></div></template>
<style scoped>.page-title{display:flex;align-items:end;justify-content:space-between;gap:12px;margin-bottom:22px}.page-title h2{font-size:24px;margin:0}.page-title p{margin:4px 0 0;color:#69736d;font-size:13px}.page-title__actions{flex:none}@media(max-width:640px){.page-title{align-items:flex-start;margin-bottom:18px}.page-title h2{font-size:21px}.page-title p{line-height:1.5}}</style>
