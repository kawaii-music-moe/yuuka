// 家計画面の入力まわり（DOM 非依存）。
import type { ReceiptDraft, Transaction } from '../api/contracts'

/** 家計簿のカテゴリ選択肢（管理画面 `frontend/src/routes/expenses/expenseUtils.ts` の EXPENSE_CATEGORIES と同じ）。 */
export const EXPENSE_CATEGORIES = ['食費', '日用品', '交通費', '光熱費', '通信費', '医療費', '娯楽', '衣服', 'その他'] as const
const FALLBACK_CATEGORY = 'その他'

/** 「取引を追加」フォームの値。 */
export type TransactionForm = Omit<Transaction, 'id'>

/** 空のフォーム。`today` は `YYYY-MM-DD`（ローカル日付）。 */
export const emptyTransactionForm = (today: string): TransactionForm =>
  ({ kind: 'expense', date: today, category: EXPENSE_CATEGORIES[0], description: '', amount: 0 })

const isCategory = (value: string) => (EXPENSE_CATEGORIES as readonly string[]).includes(value)

/** レシートの読み取り結果をフォームへ入れる。読み取れなかった日付は今の値のまま、金額は 0 にする。 */
export const applyReceiptDraft = (current: TransactionForm, draft: ReceiptDraft): TransactionForm => ({
  kind: draft.kind,
  date: draft.date ?? current.date,
  category: isCategory(draft.category) ? draft.category : FALLBACK_CATEGORY,
  description: draft.description,
  amount: draft.amount ?? 0,
})

/** 入力不足の案内（無ければ空配列）。 */
export function transactionFormErrors(form: TransactionForm): string[] {
  return [
    !form.date && '日付を入力してください。',
    !form.category && 'カテゴリを選択してください。',
    !form.description.trim() && '内容を入力してください。',
    !(form.amount > 0) && '金額は 1 円以上で入力してください。',
  ].filter((message): message is string => Boolean(message))
}
