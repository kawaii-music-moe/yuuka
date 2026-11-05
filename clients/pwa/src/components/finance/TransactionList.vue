<script setup lang="ts">
import type { Transaction } from '@/api'
import { yen } from '@/utils/format'

defineProps<{ transactions: Transaction[] }>()

/** `YYYY-MM-DD` → `MM/DD`。 */
const shortDate = (date: string) => date.slice(5).replace('-', '/')
</script>

<template>
  <section>
    <h3 class="section-title">直近の履歴</h3>
    <div class="transactions">
      <div v-for="item in transactions" :key="item.id" class="transaction">
        <time>{{ shortDate(item.date) }}</time>
        <div>
          <strong :title="item.description">{{ item.description }}</strong>
          <span>{{ item.category }}</span>
        </div>
        <b :class="item.kind">{{ item.kind === 'income' ? '+' : '-' }}{{ yen(item.amount) }}</b>
      </div>
    </div>
  </section>
</template>

<style scoped>
.transactions { border-top: 1px solid var(--line); }
.transaction { display: flex; align-items: center; gap: 14px; padding: 13px 4px; border-bottom: 1px solid var(--line); }
.transaction time { flex: none; width: 38px; font-size: 12px; color: var(--text-medium); }
/* 長い件名・カテゴリは 1 行で省略し、日付と金額は常に全体を見せる。 */
.transaction div { flex: 1; min-width: 0; display: grid; gap: 3px; }
.transaction strong, .transaction span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.transaction strong { font-size: 14px; }
.transaction span { font-size: 12px; color: var(--text-medium); }
.transaction b { flex: none; margin-left: auto; font-size: 14px; white-space: nowrap; }
.transaction b.income { color: var(--primary); }
.transaction b.expense { color: #cf6679; }
</style>
