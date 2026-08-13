<script setup lang="ts">
import { RouterLink } from 'vue-router'
import AppIcon from './AppIcon.vue'
withDefaults(defineProps<{ variant?: 'primary' | 'secondary' | 'text'; to?: string; icon?: string; type?: 'button' | 'submit'; disabled?: boolean }>(), { variant: 'primary', type: 'button' })
</script>
<template><component :is="to ? RouterLink : 'button'" :to="to" :type="type" :disabled="disabled" class="ui-button" :class="`ui-button--${variant}`"><AppIcon v-if="icon" :name="icon" /><slot /></component></template>
<style scoped>.ui-button{min-height:40px;padding:0 14px;display:inline-flex;align-items:center;justify-content:center;gap:6px;border:1px solid var(--primary);border-radius:2px;font:inherit;font-size:13px;font-weight:600;white-space:nowrap}.ui-button--primary{color:#fff;background:var(--primary)}.ui-button--secondary{color:var(--primary);background:transparent}.ui-button--text{color:var(--primary);background:transparent;border-color:transparent;padding:0 4px;min-height:32px}.ui-button:disabled{opacity:.5;cursor:not-allowed}@media(max-width:640px){.ui-button{min-height:42px}}</style>
