export const yen = (value: number) => new Intl.NumberFormat('ja-JP', { style: 'currency', currency: 'JPY', maximumFractionDigits: 0 }).format(value)
export const date = (value: string) => new Intl.DateTimeFormat('ja-JP', { month: 'numeric', day: 'numeric', weekday: 'short' }).format(new Date(value))
export const time = (value: string) => new Intl.DateTimeFormat('ja-JP', { hour: '2-digit', minute: '2-digit' }).format(new Date(value))

// `Date#toISOString()` は常に UTC 基準の `YYYY-MM-DD...` を返すため、JST（UTC+9）では
// 00:00〜08:59 の間、日付が 1 日ずれる（例: 9/24 0:00 JST → `2026-09-23T15:00:00.000Z`）。
// カレンダー・家計のように「ブラウザのローカル日付」をキーにしたい箇所は、必ずこちらを使う。
const pad2 = (value: number) => String(value).padStart(2, '0')

/** ローカルタイムゾーン基準の `YYYY-MM-DD` を返す。`toISOString().slice(0, 10)` の代替。 */
export const localDateKey = (value: Date) => `${value.getFullYear()}-${pad2(value.getMonth() + 1)}-${pad2(value.getDate())}`

/** ローカルタイムゾーン基準の `YYYY-MM` を返す。`toISOString().slice(0, 7)` の代替。 */
export const localMonthKey = (value: Date) => localDateKey(value).slice(0, 7)
