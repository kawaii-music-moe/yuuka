<script setup lang="ts">
import type { CalendarEvent } from '@/api'
import { eventsOn, monthGridDays } from '@/utils/calendar'
import { localDateKey, time } from '@/utils/format'

// 月表示のマス目（PC 用。スマホでは CalendarAgenda の一覧で見せる）。マスのクリックでその日の予定追加、
// 日付の数字のクリックでその日の日表示を依頼する。
const props = defineProps<{ anchor: Date; events: CalendarEvent[]; today: Date }>()
const emit = defineEmits<{ add: [day: Date]; selectDay: [day: Date] }>()

const WEEKDAYS = ['日', '月', '火', '水', '木', '金', '土']
const isToday = (day: Date) => localDateKey(day) === localDateKey(props.today)
</script>

<template>
  <div class="calendar">
    <div v-for="weekday in WEEKDAYS" :key="weekday" class="weekday">{{ weekday }}</div>
    <div
      v-for="day in monthGridDays(anchor)"
      :key="localDateKey(day)"
      class="day"
      :class="{ outside: day.getMonth() !== anchor.getMonth(), today: isToday(day) }"
      title="この日に予定を追加"
      @click="emit('add', day)"
    >
      <button type="button" class="date-num" title="この日を表示" @click.stop="emit('selectDay', day)">{{ day.getDate() }}</button>
      <div v-for="event in eventsOn(events, day)" :key="event.id" class="event" :style="{ borderLeftColor: event.color }">
        <span>{{ time(event.startsAt) }}</span>{{ event.title }}
      </div>
    </div>
  </div>
</template>

<style scoped>
.calendar { display: grid; grid-template-columns: repeat(7, 1fr); border-left: 1px solid var(--line); border-top: 1px solid var(--line); background: var(--surface-1); }
.weekday { font-size: 12px; color: var(--text-medium); padding: 8px; border-right: 1px solid var(--line); border-bottom: 1px solid var(--line); text-align: center; }
.day { cursor: pointer; height: 110px; border-right: 1px solid var(--line); border-bottom: 1px solid var(--line); padding: 5px; overflow: hidden; }
.day:hover { background: var(--primary-soft); }
.date-num { font-size: 12px; padding: 1px 5px; border: 0; border-radius: 2px; background: transparent; color: inherit; }
.date-num:hover { background: var(--surface-2); }
.outside { background: #121212; color: var(--text-medium); }
.today .date-num { background: var(--primary); color: #121212; }
.event { font-size: 11px; margin-top: 4px; padding: 2px 3px; border-left: 3px solid; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; background: var(--surface-2); }
.event span { color: var(--text-medium); margin-right: 3px; }
</style>
