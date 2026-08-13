<script setup lang="ts">
defineProps<{ label: string }>()
</script>
<template><label class="form-field"><span>{{ label }}</span><slot /></label></template>
<style scoped>.form-field{display:grid;gap:6px;font-size:13px;color:#4a554f}.form-field :deep(input),.form-field :deep(textarea),.form-field :deep(select){border:1px solid #aeb7b0;border-radius:2px;padding:10px;min-height:40px;color:#202522;background:#fff;font:inherit}.form-field :deep(textarea){min-height:200px;resize:vertical;line-height:1.65}</style>
