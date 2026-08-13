<script setup lang="ts">
defineProps<{ label: string }>()
</script>
<template><label class="form-field"><span>{{ label }}</span><slot /></label></template>
<style scoped>.form-field{display:grid;gap:6px;font-size:13px;color:var(--text-medium)}.form-field :deep(input),.form-field :deep(textarea),.form-field :deep(select){border:1px solid var(--field);border-radius:4px;padding:10px;min-height:40px;color:var(--text-high);background:#121212;font:inherit}.form-field :deep(textarea){min-height:200px;resize:vertical;line-height:1.65}.form-field :deep(input:focus),.form-field :deep(textarea:focus),.form-field :deep(select:focus){outline:1px solid var(--primary);border-color:var(--primary)}</style>
