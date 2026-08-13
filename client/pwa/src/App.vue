<script setup lang="ts">
import AppHeader from './components/AppHeader.vue'
import BottomNavigation from './components/BottomNavigation.vue'
</script>

<template>
  <AppHeader />
  <main class="app-main"><RouterView /></main>
  <BottomNavigation />
</template>
