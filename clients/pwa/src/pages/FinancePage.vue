<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { agentGateway, type FinanceSummary, type ReceiptDraft, type Transaction } from '@/api'
import AlertModal from '@/components/AlertModal.vue'
import PageState from '@/components/PageState.vue'
import PageTitle from '@/components/PageTitle.vue'
import FinanceSummaryBar from '@/components/finance/FinanceSummaryBar.vue'
import ReceiptScanner from '@/components/finance/ReceiptScanner.vue'
import TransactionFormView from '@/components/finance/TransactionForm.vue'
import TransactionList from '@/components/finance/TransactionList.vue'
import { applyReceiptDraft, emptyTransactionForm, transactionFormErrors } from '@/utils/finance'
import { localDateKey, localMonthKey } from '@/utils/format'

// `toISOString()` は UTC 基準のため、JST の 00:00〜08:59 は前日・前月になってしまう（issue #43）。
const month = ref(localMonthKey(new Date()))
const summary = ref<FinanceSummary>()
const transactions = ref<Transaction[]>([])
const loading = ref(true)
const error = ref('')

async function load() {
  loading.value = true
  error.value = ''
  try {
    [summary.value, transactions.value] = await Promise.all([
      agentGateway.getFinanceSummary(month.value),
      agentGateway.listTransactions(month.value),
    ])
  } catch {
    error.value = '家計データを取得できません。'
  } finally {
    loading.value = false
  }
}

const form = ref(emptyTransactionForm(localDateKey(new Date())))
const formView = ref<InstanceType<typeof TransactionFormView>>()
const addError = ref('')
const validationError = ref('')
// レシートの読み取り結果をフォームに入れた状態か（登録したら解除する）。
const receiptFilled = ref(false)

async function add() {
  const errors = transactionFormErrors(form.value)
  if (errors.length) {
    validationError.value = errors.join('\n')
    return
  }
  addError.value = ''
  try {
    await agentGateway.createTransaction(form.value)
    form.value = { ...form.value, description: '', amount: 0 }
    receiptFilled.value = false
    await load()
  } catch {
    addError.value = '取引を登録できませんでした。もう一度お試しください。'
  }
}

// 読み取った内容はフォームに入れるだけ。ユーザーが確認・修正してから「登録」する。
function handleScanned(draft: ReceiptDraft) {
  form.value = applyReceiptDraft(form.value, draft)
  receiptFilled.value = true
  formView.value?.scrollIntoView()
}

onMounted(load)
</script>

<template>
  <section>
    <PageTitle title="家計" description="収支を記録し、月ごとの傾向を把握します">
      <template #actions><input v-model="month" class="month-picker" type="month" aria-label="表示する月" @change="load" /></template>
    </PageTitle>
    <PageState :loading="loading" :error="error" />
    <template v-if="summary">
      <FinanceSummaryBar :summary="summary" />
      <ReceiptScanner :filled="receiptFilled" @scanned="handleScanned" />
      <TransactionFormView ref="formView" v-model="form" :error="addError" @submit="add" />
      <TransactionList :transactions="transactions" />
    </template>
    <AlertModal v-model="validationError" />
  </section>
</template>

<style scoped>
.month-picker { border: 1px solid var(--field); padding: 7px; border-radius: 4px; min-height: 40px; }
</style>
