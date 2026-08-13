<script setup lang="ts">
defineProps<{ name: string; label?: string }>()
</script>
<template><span class="material-symbols-outlined app-icon" :aria-label="label" :aria-hidden="label ? undefined : 'true'" :role="label ? 'img' : undefined" unselectable="on">{{ name }}</span></template>
<style scoped>.app-icon{user-select:none;-webkit-user-select:none;-moz-user-select:none}</style>
