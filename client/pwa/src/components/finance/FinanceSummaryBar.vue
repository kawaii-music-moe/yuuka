<script setup lang="ts">
import type { FinanceSummary } from '@/api'
import { yen } from '@/utils/format'

defineProps<{ summary: FinanceSummary }>()
</script>

<template>
  <div class="summary">
    <div><span>収入</span><strong class="income">{{ yen(summary.income) }}</strong></div>
    <div><span>支出</span><strong class="expense">{{ yen(summary.expense) }}</strong></div>
    <div><span>差引</span><strong>{{ yen(summary.balance) }}</strong></div>
  </div>
</template>

<style scoped>
.summary { display: grid; grid-template-columns: repeat(3, 1fr); border: 1px solid var(--line); margin-bottom: 24px; }
.summary div { padding: 14px; border-right: 1px solid var(--line); }
.summary div:last-child { border: 0; }
.summary span { display: block; font-size: 12px; color: var(--text-medium); }
.summary strong { font-size: 20px; }
.income { color: var(--primary); }
.expense { color: #cf6679; }
@media (max-width: 640px) { .summary strong { font-size: 16px; } }
</style>
