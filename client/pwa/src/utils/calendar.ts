// カレンダー画面の日付計算（DOM 非依存）。日付の比較はすべてローカル日付（`localDateKey`）で行う。
// UTC の `toISOString()` ベースで比較すると、JST の 09:00〜23:59 の予定が翌日の枠に表示されてしまう（issue #43）。
import type { CalendarEvent } from '../api/contracts'
import { localDateKey } from './format'

/** 表示の種類。 */
export type CalendarView = 'day' | 'week' | 'month'
export const CALENDAR_VIEWS: readonly CalendarView[] = ['day', 'week', 'month']

/** 時刻を落としたその日の 0 時。 */
export const startOfDay = (day: Date) => new Date(day.getFullYear(), day.getMonth(), day.getDate())
export const addDays = (day: Date, amount: number) => new Date(day.getFullYear(), day.getMonth(), day.getDate() + amount)
/** その日を含む週の日曜日。 */
export const startOfWeek = (day: Date) => addDays(day, -day.getDay())
export const startOfMonth = (day: Date) => new Date(day.getFullYear(), day.getMonth(), 1)

const range = (start: Date, length: number) => Array.from({ length }, (_, i) => addDays(start, i))

/** 月表示のマス（前後の月を含む 6 週 = 42 日）。 */
export const monthGridDays = (anchor: Date) => range(startOfWeek(startOfMonth(anchor)), 42)
/** 週表示の 7 日（日曜始まり）。 */
export const weekDays = (anchor: Date) => range(startOfWeek(anchor), 7)

/** 表示に並ぶ日（予定の取得範囲にも使う）。 */
export function visibleDays(view: CalendarView, anchor: Date): Date[] {
  if (view === 'month') return monthGridDays(anchor)
  if (view === 'week') return weekDays(anchor)
  return [startOfDay(anchor)]
}

/** 前後の期間へ移動した基準日。月表示は月初、週・日表示は 7 日・1 日ずつ動かす。 */
export function shiftAnchor(view: CalendarView, anchor: Date, amount: number): Date {
  if (view === 'month') return new Date(anchor.getFullYear(), anchor.getMonth() + amount, 1)
  return addDays(anchor, amount * (view === 'week' ? 7 : 1))
}

/** 表示中の期間が今日を含むか（月表示は前後の月のマスを除いて判定する）。 */
export function containsToday(view: CalendarView, anchor: Date, today: Date): boolean {
  if (view === 'month') return anchor.getFullYear() === today.getFullYear() && anchor.getMonth() === today.getMonth()
  return visibleDays(view, anchor).some((day) => localDateKey(day) === localDateKey(today))
}

const format = (day: Date, options: Intl.DateTimeFormatOptions) => new Intl.DateTimeFormat('ja-JP', options).format(day)

/** 期間の見出し（例: 2026年10月 / 2026年10月4日〜10日 / 2026年10月6日(火)）。 */
export function periodLabel(view: CalendarView, anchor: Date): string {
  if (view === 'month') return format(anchor, { year: 'numeric', month: 'long' })
  if (view === 'day') return format(anchor, { year: 'numeric', month: 'long', day: 'numeric', weekday: 'short' })
  const days = weekDays(anchor)
  const [first, last] = [days[0], days[6]]
  const lastOptions: Intl.DateTimeFormatOptions = first.getFullYear() !== last.getFullYear()
    ? { year: 'numeric', month: 'long', day: 'numeric' }
    : first.getMonth() !== last.getMonth() ? { month: 'long', day: 'numeric' } : { day: 'numeric' }
  return `${format(first, { year: 'numeric', month: 'long', day: 'numeric' })}〜${format(last, lastOptions)}`
}

export const weekdayLabel = (day: Date) => format(day, { weekday: 'short' })

const byStart = (a: CalendarEvent, b: CalendarEvent) => a.startsAt.localeCompare(b.startsAt)

/** その日に始まる予定（開始時刻順）。 */
export const eventsOn = (events: CalendarEvent[], day: Date) =>
  events.filter((event) => localDateKey(new Date(event.startsAt)) === localDateKey(day)).sort(byStart)

export type AgendaDay = { day: Date; events: CalendarEvent[] }

/**
 * スマホ向けの日ごとの一覧。週表示は予定の無い日も含めて 7 日分、月表示は基準日の月で予定のある日だけ。
 * 日表示は時間軸で見せるので使わない。
 */
export function agendaDays(view: CalendarView, anchor: Date, events: CalendarEvent[]): AgendaDay[] {
  if (view === 'week') return weekDays(anchor).map((day) => ({ day, events: eventsOn(events, day) }))
  const groups = new Map<string, AgendaDay>()
  for (const event of [...events].sort(byStart)) {
    const day = startOfDay(new Date(event.startsAt))
    if (day.getFullYear() !== anchor.getFullYear() || day.getMonth() !== anchor.getMonth()) continue
    const group = groups.get(localDateKey(day)) ?? { day, events: [] }
    group.events.push(event)
    groups.set(localDateKey(day), group)
  }
  return [...groups.values()]
}

/**
 * 予定に表示するカレンダー名。`calendar` には生の Google カレンダー ID（`…@group.calendar.google.com`）が
 * 入り得るので、人間可読の `calendarName` を優先し、ID しか無ければ出さない。
 */
export const calendarLabel = (event: CalendarEvent) => event.calendarName || (event.calendar.includes('@') ? '' : event.calendar)

const DAY_MINUTES = 24 * 60
/** 短すぎる予定も読めるよう、描画上の最小の長さ（分）。 */
export const MIN_EVENT_MINUTES = 30

/** 時間軸上の予定の配置。`start`/`end` はその日の 0 時からの分、`lane`/`lanes` は重なりの横分割。 */
export type TimedEvent = { event: CalendarEvent; start: number; end: number; lane: number; lanes: number }

const minutesOf = (value: Date) => value.getHours() * 60 + value.getMinutes()

/**
 * 時間軸に並べる予定の配置を求める。重なり合う予定のまとまりごとに、同時に重なる最大数で横に等分する。
 * 翌日以降に終わる予定はその日の終わりまで描く。
 */
export function layoutDayEvents(events: CalendarEvent[], day: Date): TimedEvent[] {
  const dayKey = localDateKey(day)
  const items = eventsOn(events, day)
    .map((event) => {
      const start = minutesOf(new Date(event.startsAt))
      const endDate = new Date(event.endsAt)
      const end = localDateKey(endDate) === dayKey ? minutesOf(endDate) : DAY_MINUTES
      return { event, start, end: Math.min(DAY_MINUTES, Math.max(end, start + MIN_EVENT_MINUTES)) }
    })
    .sort((a, b) => a.start - b.start || b.end - a.end)

  const placed: TimedEvent[] = []
  let cluster: TimedEvent[] = []
  let laneEnds: number[] = []
  let clusterEnd = -1
  const flush = () => {
    for (const item of cluster) placed.push({ ...item, lanes: Math.max(1, laneEnds.length) })
    cluster = []; laneEnds = []; clusterEnd = -1
  }
  for (const item of items) {
    if (item.start >= clusterEnd) flush()
    let lane = laneEnds.findIndex((end) => end <= item.start)
    if (lane === -1) { lane = laneEnds.length; laneEnds.push(item.end) } else laneEnds[lane] = item.end
    cluster.push({ ...item, lane, lanes: 1 })
    clusterEnd = Math.max(clusterEnd, item.end)
  }
  flush()
  return placed
}
