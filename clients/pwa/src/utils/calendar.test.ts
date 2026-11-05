// `npm test`（tsx --test）で実行する。予定の日時はオフセット無しの ISO（= ローカル時刻）で与える。
import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import type { CalendarEvent } from '../api/contracts'
import { agendaDays, containsToday, layoutDayEvents, monthGridDays, periodLabel, shiftAnchor, visibleDays } from './calendar'
import { localDateKey } from './format'

const keys = (days: Date[]) => days.map(localDateKey)
const event = (id: string, startsAt: string, endsAt = startsAt): CalendarEvent =>
  ({ id, title: id, startsAt, endsAt, calendar: 'Personal', calendarName: 'Personal', color: '#155eef' })

// 2026-10-06 は火曜日。
const anchor = new Date(2026, 9, 6)

describe('visibleDays', () => {
  it('returns the Sunday-start week and the single day', () => {
    assert.deepEqual(keys(visibleDays('week', anchor)), ['2026-10-04', '2026-10-05', '2026-10-06', '2026-10-07', '2026-10-08', '2026-10-09', '2026-10-10'])
    assert.deepEqual(keys(visibleDays('day', new Date(2026, 9, 6, 15, 30))), ['2026-10-06'])
  })

  it('returns a 6-week month grid starting on the Sunday before the 1st', () => {
    const days = monthGridDays(anchor)
    assert.equal(days.length, 42)
    assert.equal(localDateKey(days[0]), '2026-09-27')
    assert.equal(localDateKey(days[41]), '2026-11-07')
  })
})

describe('shiftAnchor / containsToday', () => {
  it('moves by month, week and day', () => {
    assert.equal(localDateKey(shiftAnchor('month', anchor, 1)), '2026-11-01')
    assert.equal(localDateKey(shiftAnchor('month', new Date(2026, 0, 31), -1)), '2025-12-01')
    assert.equal(localDateKey(shiftAnchor('week', anchor, -1)), '2026-09-29')
    assert.equal(localDateKey(shiftAnchor('day', anchor, 1)), '2026-10-07')
  })

  it('ignores the neighbouring-month cells in the month view', () => {
    const today = new Date(2026, 9, 1)
    assert.equal(containsToday('month', anchor, today), true)
    // 9 月の月表示のマスには 10/1 も並ぶが、今日を含む期間とはみなさない。
    assert.equal(containsToday('month', new Date(2026, 8, 15), today), false)
    assert.equal(containsToday('week', new Date(2026, 8, 28), today), true)
  })
})

describe('periodLabel', () => {
  it('formats each view', () => {
    assert.equal(periodLabel('month', anchor), '2026年10月')
    assert.equal(periodLabel('day', anchor), '2026年10月6日(火)')
    assert.equal(periodLabel('week', anchor), '2026年10月4日〜10日')
    assert.equal(periodLabel('week', new Date(2026, 8, 30)), '2026年9月27日〜10月3日')
    assert.equal(periodLabel('week', new Date(2026, 11, 31)), '2026年12月27日〜2027年1月2日')
  })
})

describe('agendaDays', () => {
  const events = [event('b', '2026-10-06T12:00:00'), event('a', '2026-10-06T09:00:00'), event('next-month', '2026-11-01T09:00:00')]

  it('lists all 7 days for the week view, including empty days', () => {
    const days = agendaDays('week', anchor, events)
    assert.equal(days.length, 7)
    assert.deepEqual(days[2].events.map((e) => e.id), ['a', 'b'])
    assert.equal(days[0].events.length, 0)
  })

  it('lists only days with events in the anchor month for the month view', () => {
    const days = agendaDays('month', anchor, events)
    assert.deepEqual(days.map((d) => localDateKey(d.day)), ['2026-10-06'])
  })
})

describe('layoutDayEvents', () => {
  it('splits overlapping events into lanes and keeps separate ones full width', () => {
    const placed = layoutDayEvents([
      event('a', '2026-10-06T09:00:00', '2026-10-06T10:00:00'),
      event('b', '2026-10-06T09:30:00', '2026-10-06T11:00:00'),
      event('c', '2026-10-06T13:00:00', '2026-10-06T14:00:00'),
    ], anchor)
    const byId = Object.fromEntries(placed.map((p) => [p.event.id, p]))
    assert.deepEqual([byId.a.lane, byId.a.lanes], [0, 2])
    assert.deepEqual([byId.b.lane, byId.b.lanes], [1, 2])
    assert.deepEqual([byId.c.lane, byId.c.lanes], [0, 1])
    assert.deepEqual([byId.a.start, byId.a.end], [540, 600])
  })

  it('gives short events a minimum height and clips events ending on a later day', () => {
    const placed = layoutDayEvents([
      event('point', '2026-10-06T08:00:00'),
      event('overnight', '2026-10-06T23:00:00', '2026-10-07T01:00:00'),
      event('other-day', '2026-10-07T09:00:00'),
    ], anchor)
    const byId = Object.fromEntries(placed.map((p) => [p.event.id, p]))
    assert.deepEqual([byId.point.start, byId.point.end], [480, 510])
    assert.deepEqual([byId.overnight.start, byId.overnight.end], [1380, 1440])
    assert.equal(byId['other-day'], undefined)
  })
})
