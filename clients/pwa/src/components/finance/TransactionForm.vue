<script setup lang="ts">
import { ref } from 'vue'
import FormField from '../FormField.vue'
import UiButton from '../UiButton.vue'
import { EXPENSE_CATEGORIES, type TransactionForm } from '@/utils/finance'

// 「取引を追加」フォーム。入力の検証と登録は親が行う（`submit`）。レシート読み取り後に親がここまでスクロールする。
const form = defineModel<TransactionForm>({ required: true })
defineProps<{ error: string }>()
const emit = defineEmits<{ submit: [] }>()

const root = ref<HTMLFormElement>()
defineExpose({ scrollIntoView: () => root.value?.scrollIntoView({ behavior: 'smooth', block: 'start' }) })
</script>

<template>
  <form ref="root" class="transaction-form" novalidate @submit.prevent="emit('submit')">
    <h3>取引を追加</h3>
    <FormField label="種別"><select v-model="form.kind"><option value="expense">支出</option><option value="income">収入</option></select></FormField>
    <FormField label="日付"><input v-model="form.date" type="date" /></FormField>
    <FormField label="カテゴリ"><select v-model="form.category"><option v-for="category in EXPENSE_CATEGORIES" :key="category" :value="category">{{ category }}</option></select></FormField>
    <FormField label="内容" class="wide"><input v-model="form.description" placeholder="例: 昼食" /></FormField>
    <FormField label="金額" class="wide"><input v-model.number="form.amount" type="number" min="1" /></FormField>
    <UiButton type="submit" class="wide">登録</UiButton>
    <p v-if="error" class="error add-error">{{ error }}</p>
  </form>
</template>

<style scoped>
/* スクロールで寄せたとき、固定ヘッダーの下に隠れないようにする。 */
.transaction-form { scroll-margin-top: 72px; padding: 16px 0 20px; border-top: 1px solid var(--line); border-bottom: 1px solid var(--line); display: grid; grid-template-columns: repeat(5, 1fr) auto; gap: 10px; align-items: end; margin-bottom: 28px; }
.transaction-form h3 { grid-column: 1 / -1; margin: 0; font-size: 14px; }
.add-error { grid-column: 1 / -1; margin: 0; font-size: 12px; }
/* スマホは 2 列。種別・日付・カテゴリ以外は幅いっぱいにする。 */
@media (max-width: 640px) {
  .transaction-form { grid-template-columns: 1fr 1fr; }
  .transaction-form > :nth-child(4) { grid-column: span 2; }
  .wide { grid-column: span 2; }
}
</style>
