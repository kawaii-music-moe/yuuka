<script setup lang="ts">
import { calendarLabel, weekdayLabel, type AgendaDay } from '@/utils/calendar'
import { localDateKey, time } from '@/utils/format'

// スマホ向けの日ごとの予定一覧（月・週表示）。予定の無い日（週表示）からはその日の予定追加を依頼できる。
const props = defineProps<{ days: AgendaDay[]; today: Date }>()
const emit = defineEmits<{ add: [day: Date]; selectDay: [day: Date] }>()

const todayKey = () => localDateKey(props.today)
</script>

<template>
  <section class="agenda">
    <p v-if="!days.length" class="agenda-empty">予定はありません</p>
    <div
      v-for="group in days"
      :key="localDateKey(group.day)"
      class="agenda-day"
      :class="{ today: localDateKey(group.day) === todayKey(), past: localDateKey(group.day) < todayKey() }"
    >
      <button type="button" class="agenda-date" title="この日を表示" @click="emit('selectDay', group.day)">
        <strong>{{ group.day.getDate() }}</strong><span>{{ weekdayLabel(group.day) }}</span>
      </button>
      <ul>
        <li v-for="event in group.events" :key="event.id" class="agenda-item" :style="{ borderLeftColor: event.color }">
          <time>{{ time(event.startsAt) }}</time>
          <div><strong>{{ event.title }}</strong><small v-if="calendarLabel(event)">{{ calendarLabel(event) }}</small></div>
        </li>
        <li v-if="!group.events.length" class="agenda-none"><button type="button" @click="emit('add', group.day)">予定なし（追加する）</button></li>
      </ul>
    </div>
  </section>
</template>

<style scoped>
.agenda-empty { color: var(--text-medium); font-size: 13px; padding: 16px 0; }
.agenda-day { display: flex; gap: 12px; padding: 12px 0; border-top: 1px solid var(--line); }
.agenda-day.past { opacity: .55; }
.agenda-date { width: 36px; flex: none; display: grid; justify-items: center; align-content: start; line-height: 1.2; padding: 0; border: 0; background: transparent; color: var(--text-high); }
.agenda-date strong { font-size: 20px; }
.agenda-date span { font-size: 11px; color: var(--text-medium); }
.agenda-day.today .agenda-date strong { color: var(--primary); }
.agenda-day ul { flex: 1; min-width: 0; list-style: none; margin: 0; padding: 0; display: grid; gap: 8px; }
.agenda-item { display: flex; gap: 10px; align-items: baseline; min-width: 0; padding: 8px 10px; background: var(--surface-1); border: 1px solid var(--line); border-left: 3px solid; border-radius: 4px; }
.agenda-item time { flex: none; font-size: 12px; color: var(--text-medium); font-variant-numeric: tabular-nums; }
.agenda-item div { min-width: 0; display: grid; gap: 2px; }
.agenda-item strong { font-size: 14px; overflow-wrap: anywhere; }
.agenda-item small { font-size: 11px; color: var(--text-medium); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.agenda-none button { padding: 6px 0; border: 0; background: transparent; color: var(--text-medium); font-size: 13px; }
</style>
