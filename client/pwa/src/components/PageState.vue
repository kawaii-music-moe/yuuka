<script setup lang="ts">
defineProps<{ loading?: boolean; error?: string }>()
</script>
<template><p v-if="loading" class="state-text">読み込み中です。</p><p v-else-if="error" class="state-text error">{{ error }}</p></template>
