<script setup lang="ts">
import AppHeader from './AppHeader.vue'
import BottomNavigation from './BottomNavigation.vue'
</script>

<template>
  <AppHeader />
  <main class="app-main"><slot /></main>
  <BottomNavigation />
</template>
