<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { agentGateway, type FinanceSummary, type Transaction } from '@/api'
import PageState from '@/components/PageState.vue'
import { localDateKey, localMonthKey, yen } from '@/utils/format'
import FormField from '@/components/FormField.vue'
import PageTitle from '@/components/PageTitle.vue'
import UiButton from '@/components/UiButton.vue'
import AlertModal from '@/components/AlertModal.vue'
import { ApiError } from '@/api/http'
import { prepareReceiptImage, RECEIPT_MIME_TYPES } from '@/utils/receiptImage'
// `toISOString()` は UTC 基準のため、JST の 00:00〜08:59 は前日・前月になってしまう（issue #43）。
const month = ref(localMonthKey(new Date())); const summary = ref<FinanceSummary>(); const transactions = ref<Transaction[]>([]); const loading = ref(true); const error = ref(''); const addError = ref(''); const validationError = ref(''); const form = ref({ date: localDateKey(new Date()), category: '食費', description: '', amount: 0, kind: 'expense' as 'income' | 'expense' })
async function load() { loading.value = true; try { [summary.value, transactions.value] = await Promise.all([agentGateway.getFinanceSummary(month.value), agentGateway.listTransactions(month.value)]) } catch { error.value = '家計データを取得できません。' } finally { loading.value = false } }
async function add() {
  const missing = [
    !form.value.date && '日付を入力してください。',
    !form.value.category.trim() && 'カテゴリを入力してください。',
    !form.value.description.trim() && '内容を入力してください。',
    !(form.value.amount > 0) && '金額は 1 円以上で入力してください。',
  ].filter(Boolean)
  if (missing.length) { validationError.value = missing.join('\n'); return }
  addError.value = ''
  try {
    const entry = await agentGateway.createTransaction(form.value)
    transactions.value.unshift(entry)
    await load()
    form.value.description = ''
    form.value.amount = 0
  } catch {
    addError.value = '取引を登録できませんでした。もう一度お試しください。'
  }
}

const scanning = ref(false); const scanError = ref(''); const scanResult = ref(''); const receiptInput = ref<HTMLInputElement>()
async function scanReceipt(event: Event) {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  input.value = '' // 同じ画像を選び直しても change が発火するようにする
  if (!file) return
  // 拡張子だけで MIME が空になる端末もあるため、空なら送ってサーバーの判定に任せる。
  if (file.type && !RECEIPT_MIME_TYPES.includes(file.type.toLowerCase())) { validationError.value = 'PNG・JPEG・WebP・HEIC・GIF の画像を選んでください。'; return }
  scanning.value = true; scanError.value = ''
  try {
    const result = await agentGateway.scanReceipt(await prepareReceiptImage(file))
    scanResult.value = result.response || 'レシートを読み取りました。'
    await load()
  } catch (cause) {
    scanError.value = cause instanceof ApiError && cause.serverMessage ? cause.serverMessage : 'レシートを読み取れませんでした。もう一度お試しください。'
  } finally {
    scanning.value = false
  }
}
onMounted(load)
</script>
<template><section><PageTitle title="家計" description="収支を記録し、月ごとの傾向を把握します"><template #actions><input v-model="month" class="month-picker" type="month" @change="load" /></template></PageTitle><PageState :loading="loading" :error="error"/><template v-if="summary"><div class="summary"><div><span>収入</span><strong class="income">{{ yen(summary.income) }}</strong></div><div><span>支出</span><strong class="expense">{{ yen(summary.expense) }}</strong></div><div><span>差引</span><strong>{{ yen(summary.balance) }}</strong></div></div><section class="receipt surface"><div><h3>レシートから記録</h3><p>レシートの写真を選ぶと、エージェントが内容を読み取って記録します。</p><p v-if="scanError" class="error scan-error">{{ scanError }}</p></div><UiButton variant="secondary" icon="receipt_long" :disabled="scanning" @click="receiptInput?.click()">{{ scanning ? '読み取り中…' : 'レシート画像を選択' }}</UiButton><input ref="receiptInput" class="receipt-input" type="file" accept="image/*" @change="scanReceipt" /></section><form class="transaction-form surface" novalidate @submit.prevent="add"><h3>取引を追加</h3><FormField label="種別"><select v-model="form.kind"><option value="expense">支出</option><option value="income">収入</option></select></FormField><FormField label="日付"><input v-model="form.date" type="date" /></FormField><FormField label="カテゴリ"><input v-model="form.category" /></FormField><FormField label="内容"><input v-model="form.description" placeholder="例: 昼食" /></FormField><FormField label="金額"><input v-model.number="form.amount" type="number" min="1" /></FormField><UiButton type="submit">登録</UiButton><p v-if="addError" class="error add-error">{{ addError }}</p></form><section><h3 class="section-title">直近の履歴</h3><div class="transactions"><div v-for="item in transactions" :key="item.id" class="transaction"><time>{{ item.date.slice(5).replace('-', '/') }}</time><div><strong>{{ item.description }}</strong><span>{{ item.category }}</span></div><b :class="item.kind">{{ item.kind === 'income' ? '+' : '-' }}{{ yen(item.amount) }}</b></div></div></section></template><AlertModal v-model="validationError" /><AlertModal v-model="scanResult" title="レシートの読み取り結果" /></section></template>
<style scoped>.month-picker{border:1px solid var(--field);padding:7px;border-radius:4px;min-height:40px}.summary{display:grid;grid-template-columns:repeat(3,1fr);border:1px solid var(--line);margin-bottom:24px}.summary div{padding:14px;border-right:1px solid var(--line)}.summary div:last-child{border:0}.summary span{display:block;font-size:12px;color:var(--text-medium)}.summary strong{font-size:20px}.income{color:var(--primary)}.expense{color:#cf6679}.receipt{padding:16px;display:flex;align-items:center;justify-content:space-between;gap:16px;margin-bottom:16px}.receipt h3{margin:0;font-size:14px}.receipt p{margin:4px 0 0;font-size:12px;color:var(--text-medium)}.receipt .scan-error{color:#cf6679}.receipt-input{display:none}.transaction-form{padding:16px;display:grid;grid-template-columns:repeat(5,1fr) auto;gap:10px;align-items:end;margin-bottom:28px}.transaction-form h3{grid-column:1/-1;margin:0;font-size:14px}.add-error{grid-column:1/-1;margin:0;font-size:12px}.transactions{border-top:1px solid var(--line)}.transaction{display:flex;align-items:center;gap:14px;padding:13px 4px;border-bottom:1px solid var(--line)}.transaction time{font-size:12px;color:var(--text-medium);width:38px}.transaction div{display:grid;gap:3px}.transaction strong{font-size:14px}.transaction span{font-size:12px;color:var(--text-medium)}.transaction b{margin-left:auto;font-size:14px}.transaction b.income{color:var(--primary)}.transaction b.expense{color:#cf6679}@media(max-width:640px){.summary strong{font-size:16px}.receipt{flex-direction:column;align-items:stretch}.transaction-form{grid-template-columns:1fr 1fr}.transaction-form h3{grid-column:1/-1}.transaction-form .form-field:nth-child(4),.transaction-form .form-field:nth-child(5),.transaction-form .ui-button{grid-column:span 2}}</style>
