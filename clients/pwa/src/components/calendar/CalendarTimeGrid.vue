<script setup lang="ts">
import { computed } from 'vue'
import type { CalendarEvent } from '@/api'
import { layoutDayEvents, weekdayLabel } from '@/utils/calendar'
import { localDateKey, time } from '@/utils/format'

// 週・日表示の時間軸グリッド。列＝日、縦＝0〜24 時。空き枠をクリックすると、その日時で予定追加を依頼する。
// スクロールはページ側の 1 本だけにする（グリッド内では縦スクロールしない）。
// 日表示では日付がページ上部の表示と重なるので、見出し行を出さない（`showHeader`）。
const props = withDefaults(defineProps<{ days: Date[]; events: CalendarEvent[]; today: Date; showHeader?: boolean }>(), { showHeader: true })
const emit = defineEmits<{ add: [dateTime: string]; selectDay: [day: Date] }>()

const HOUR_HEIGHT = 48
const hours = Array.from({ length: 24 }, (_, hour) => hour)

const columns = computed(() => props.days.map((day) => ({
  day,
  key: localDateKey(day),
  events: layoutDayEvents(props.events, day).map((placed) => ({
    event: placed.event,
    style: {
      top: `${(placed.start / 60) * HOUR_HEIGHT}px`,
      height: `${((placed.end - placed.start) / 60) * HOUR_HEIGHT}px`,
      left: `${(placed.lane / placed.lanes) * 100}%`,
      width: `calc(${100 / placed.lanes}% - 2px)`,
      borderLeftColor: placed.event.color,
    },
  })),
})))

function handleColumnClick(day: Date, event: MouseEvent) {
  const column = event.currentTarget as HTMLElement
  const offset = event.clientY - column.getBoundingClientRect().top
  const hour = Math.min(23, Math.max(0, Math.floor(offset / HOUR_HEIGHT)))
  emit('add', `${localDateKey(day)}T${String(hour).padStart(2, '0')}:00`)
}
</script>

<template>
  <div class="time-grid" :style="{ '--days': days.length, '--hour-height': `${HOUR_HEIGHT}px` }">
    <div v-if="showHeader" class="grid-header">
      <div class="corner"></div>
      <button
        v-for="column in columns"
        :key="column.key"
        type="button"
        class="day-heading"
        :class="{ today: column.key === localDateKey(today) }"
        :title="days.length > 1 ? 'この日を表示' : undefined"
        @click="emit('selectDay', column.day)"
      >
        <span>{{ weekdayLabel(column.day) }}</span>
        <strong>{{ column.day.getDate() }}</strong>
      </button>
    </div>
    <div class="grid-body">
      <div class="hour-labels">
        <span v-for="hour in hours" :key="hour">{{ hour === 0 ? '' : `${hour}:00` }}</span>
      </div>
      <div
        v-for="column in columns"
        :key="column.key"
        class="day-column"
        title="この時間に予定を追加"
        @click="handleColumnClick(column.day, $event)"
      >
        <div
          v-for="placed in column.events"
          :key="placed.event.id"
          class="grid-event"
          :style="placed.style"
          :title="`${time(placed.event.startsAt)} ${placed.event.title}`"
          @click.stop
        >
          <strong>{{ placed.event.title }}</strong>
          <span>{{ time(placed.event.startsAt) }}〜{{ time(placed.event.endsAt) }}</span>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* カードにせずページ背景に馴染ませ、罫線だけで区切る。 */
.time-grid { --grid-line: rgba(255,255,255,.07); }
.grid-header, .grid-body { display: grid; grid-template-columns: 44px repeat(var(--days), minmax(0, 1fr)); }
/* 日付の見出しはアプリのヘッダーの下に貼り付ける。 */
.grid-header { position: sticky; top: 58px; z-index: 2; background: #121212; border-bottom: 1px solid var(--line); }
.day-heading { display: flex; align-items: baseline; justify-content: center; gap: 6px; padding: 8px 0; border: 0; background: transparent; color: var(--text-medium); font-size: 12px; }
.day-heading strong { font-size: 18px; color: var(--text-high); line-height: 1.2; }
.day-heading.today strong { color: var(--primary); }
.hour-labels span { display: block; height: var(--hour-height); padding-right: 8px; text-align: right; font-size: 11px; color: var(--text-medium); transform: translateY(-7px); }
.day-column { position: relative; height: calc(var(--hour-height) * 24); border-left: 1px solid var(--grid-line); cursor: pointer; background-image: linear-gradient(to bottom, var(--grid-line) 1px, transparent 1px); background-size: 100% var(--hour-height); }
.day-column:hover { background-color: rgba(255,255,255,.02); }
.grid-event { position: absolute; display: grid; align-content: start; gap: 1px; overflow: hidden; padding: 3px 6px; border-left: 3px solid; border-radius: 3px; background: var(--primary-soft); font-size: 11px; cursor: default; }
.grid-event strong { font-size: 12px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.grid-event span { color: var(--text-medium); white-space: nowrap; }
@media (max-width: 640px) {
  .grid-header { top: 54px; }
  .grid-header, .grid-body { grid-template-columns: 36px repeat(var(--days), minmax(0, 1fr)); }
  .hour-labels span { font-size: 10px; padding-right: 4px; }
}
</style>
