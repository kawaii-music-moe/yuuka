<script setup lang="ts">
defineProps<{ title?: string; description?: string }>()
</script>

<template>
  <section class="content-section">
    <header v-if="title || description" class="content-section__header">
      <div><h3 v-if="title">{{ title }}</h3><p v-if="description">{{ description }}</p></div>
      <slot name="actions" />
    </header>
    <div class="content-section__body"><slot /></div>
  </section>
</template>

<style scoped>
.content-section { border-top: 1px solid var(--border-divider); padding: 20px 0; }
.content-section__header { display: flex; align-items: start; justify-content: space-between; gap: 16px; margin-bottom: 16px; }
h3 { margin: 0; font-size: 15px; color: var(--text-high); } p { margin: 5px 0 0; color: var(--text-medium); font-size: 13px; }
</style>
