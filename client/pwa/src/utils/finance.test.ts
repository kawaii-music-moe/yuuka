// `npm test`（tsx --test）で実行する。
import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { applyReceiptDraft, emptyTransactionForm, transactionFormErrors } from './finance'

describe('applyReceiptDraft', () => {
  const current = { ...emptyTransactionForm('2026-10-06'), description: '書きかけ', amount: 500 }

  it('replaces the form with the scanned values', () => {
    const form = applyReceiptDraft(current, { date: '2026-09-11', kind: 'expense', category: '日用品', description: 'サミット: 卵', amount: 600 })
    assert.deepEqual(form, { kind: 'expense', date: '2026-09-11', category: '日用品', description: 'サミット: 卵', amount: 600 })
  })

  it('keeps the date and clears the amount when they could not be read', () => {
    const form = applyReceiptDraft(current, { date: null, kind: 'income', category: '外食', description: '', amount: null })
    assert.deepEqual(form, { kind: 'income', date: '2026-10-06', category: 'その他', description: '', amount: 0 })
  })
})

describe('transactionFormErrors', () => {
  it('accepts a complete form', () => {
    assert.deepEqual(transactionFormErrors({ ...emptyTransactionForm('2026-10-06'), description: '昼食', amount: 900 }), [])
  })

  it('lists every missing field', () => {
    assert.deepEqual(transactionFormErrors({ ...emptyTransactionForm(''), description: '  ', amount: 0 }), [
      '日付を入力してください。',
      '内容を入力してください。',
      '金額は 1 円以上で入力してください。',
    ])
  })
})
