<script setup lang="ts">
defineProps<{ label: string }>()
</script>
<template><label class="form-field"><span>{{ label }}</span><slot /></label></template>
<style scoped>.form-field{display:grid;gap:6px;font-size:13px;color:var(--text-medium)}.form-field :deep(textarea){min-height:200px;resize:vertical;line-height:1.65}</style>
