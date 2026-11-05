<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { agentGateway, type CalendarEvent } from '@/api'
import PageState from '@/components/PageState.vue'
import UiButton from '@/components/UiButton.vue'
import CalendarAgenda from '@/components/calendar/CalendarAgenda.vue'
import CalendarEventModal from '@/components/calendar/CalendarEventModal.vue'
import CalendarMonthGrid from '@/components/calendar/CalendarMonthGrid.vue'
import CalendarTimeGrid from '@/components/calendar/CalendarTimeGrid.vue'
import CalendarViewMenu from '@/components/calendar/CalendarViewMenu.vue'
import {
  agendaDays, CALENDAR_VIEWS, containsToday, periodLabel, shiftAnchor, startOfDay, startOfMonth, visibleDays,
  type CalendarView,
} from '@/utils/calendar'
import { localDateKey } from '@/utils/format'

const VIEW_STORAGE_KEY = 'yuuka.pwa.calendarView'

function loadStoredView(): CalendarView {
  try {
    const stored = window.localStorage.getItem(VIEW_STORAGE_KEY) as CalendarView | null
    return stored && CALENDAR_VIEWS.includes(stored) ? stored : 'month'
  } catch {
    return 'month'
  }
}

function storeView(view: CalendarView) {
  try {
    window.localStorage.setItem(VIEW_STORAGE_KEY, view)
  } catch {
    // 保存できなくても、このタブの中では切り替わったまま動く。
  }
}

const today = new Date()
const view = ref<CalendarView>(loadStoredView())
// 表示の基準日。月表示はこの日を含む月、週表示はこの日を含む週（日曜始まり）、日表示はこの日。
const anchor = ref(startOfDay(today))
const events = ref<CalendarEvent[]>([])
const loading = ref(true)
const error = ref('')

const days = computed(() => visibleDays(view.value, anchor.value))
const label = computed(() => periodLabel(view.value, anchor.value))
const isCurrentPeriod = computed(() => containsToday(view.value, anchor.value, today))
const agenda = computed(() => agendaDays(view.value, anchor.value, events.value))

// 素早く期間を移動したとき、古い応答で新しい表示を上書きしないよう最新の読み込みだけを反映する。
let loadSeq = 0
async function load() {
  const seq = ++loadSeq
  loading.value = true
  error.value = ''
  try {
    const range = days.value
    const result = await agentGateway.listCalendarEvents(localDateKey(range[0]), localDateKey(range[range.length - 1]))
    if (seq === loadSeq) events.value = result
  } catch {
    if (seq === loadSeq) error.value = 'カレンダーを取得できません。'
  } finally {
    if (seq === loadSeq) loading.value = false
  }
}

function setView(next: CalendarView) {
  view.value = next
  storeView(next)
  // 表示を切り替えたら、どの表示でもページの先頭から見せる。
  window.scrollTo({ top: 0 })
  load()
}
function move(amount: number) {
  anchor.value = shiftAnchor(view.value, anchor.value, amount)
  load()
}
function goToday() {
  anchor.value = startOfDay(today)
  load()
}
function showDay(day: Date) {
  anchor.value = day
  setView('day')
}

// 予定追加フォームを開く日時（空なら閉じている）。日付だけなら 9 時、日時ならその時刻から。
// 既定は表示中の期間に今日があれば今日、なければ期間の最初の日。
const addingDate = ref('')
function openAdd(target?: Date | string) {
  if (typeof target === 'string') {
    addingDate.value = target
    return
  }
  const fallback = isCurrentPeriod.value ? today : view.value === 'month' ? startOfMonth(anchor.value) : anchor.value
  addingDate.value = localDateKey(target ?? fallback)
}
function handleCreated(event: CalendarEvent) {
  events.value.push(event)
}

onMounted(load)
</script>

<template>
  <section class="calendar-page">
    <div class="calendar-controls">
      <div class="nav">
        <UiButton variant="secondary" icon="chevron_left" aria-label="前へ" @click="move(-1)" />
        <UiButton variant="secondary" :disabled="isCurrentPeriod" @click="goToday">今日</UiButton>
        <UiButton variant="secondary" icon="chevron_right" aria-label="次へ" @click="move(1)" />
      </div>
      <strong class="period-label">{{ label }}</strong>
      <CalendarViewMenu :model-value="view" class="view-menu" @update:model-value="setView" />
      <UiButton class="add-button" icon="add" @click="openAdd()">予定を追加</UiButton>
    </div>
    <PageState :loading="loading" :error="error" />
    <template v-if="!loading && !error">
      <CalendarMonthGrid v-if="view === 'month'" class="desktop-only" :anchor="anchor" :events="events" :today="today" @add="openAdd" @select-day="showDay" />
      <CalendarTimeGrid
        v-else
        :class="{ 'desktop-only': view === 'week' }"
        :days="days"
        :events="events"
        :today="today"
        :show-header="view === 'week'"
        @add="openAdd"
        @select-day="showDay"
      />
      <CalendarAgenda v-if="view !== 'day'" class="mobile-only" :days="agenda" :today="today" @add="openAdd" @select-day="showDay" />
    </template>
    <CalendarEventModal v-model:date="addingDate" @created="handleCreated" />
  </section>
</template>

<style scoped>
/* PC は 1 行（移動・期間 … 表示切替・追加）、スマホは 2 行（期間・表示切替 / 移動・追加）。 */
.calendar-controls { display: grid; grid-template-columns: auto minmax(0, 1fr) auto auto; grid-template-areas: "nav label view add"; align-items: center; gap: 8px 12px; margin-bottom: 14px; }
.nav { grid-area: nav; display: flex; align-items: center; gap: 6px; }
.nav .ui-button { padding: 0 10px; }
.period-label { grid-area: label; font-size: 16px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.view-menu { grid-area: view; }
.add-button { grid-area: add; }
/* 月・週の格子は PC だけ、日ごとの一覧はスマホだけ（日表示は両方とも時間軸）。子コンポーネントの
   display 指定より優先させるため、ページ直下の指定として詳細度を上げる。 */
.calendar-page > .mobile-only { display: none; }
@media (max-width: 640px) {
  .calendar-controls { grid-template-columns: auto minmax(0, 1fr) auto; grid-template-areas: "label label view" "nav . add"; }
  .period-label { font-size: 18px; }
  .calendar-page > .desktop-only { display: none; }
  .calendar-page > .mobile-only { display: block; }
}
</style>
