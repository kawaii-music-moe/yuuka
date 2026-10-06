<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from 'vue'
import AppIcon from '../AppIcon.vue'
import type { CalendarView } from '@/utils/calendar'

// 表示切り替えのドロップダウン（日・週・月）。右端のキー（D / W / M）はキーボードショートカット。
const VIEWS: { value: CalendarView; label: string; shortcut: string }[] = [
  { value: 'day', label: '日', shortcut: 'D' },
  { value: 'week', label: '週', shortcut: 'W' },
  { value: 'month', label: '月', shortcut: 'M' },
]

const view = defineModel<CalendarView>({ required: true })
const open = ref(false)
const root = ref<HTMLElement>()

function choose(next: CalendarView) {
  view.value = next
  open.value = false
}

function isTyping(target: EventTarget | null) {
  const element = target as HTMLElement | null
  return !!element && (['INPUT', 'TEXTAREA', 'SELECT'].includes(element.tagName) || element.isContentEditable)
}

function handleKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') { open.value = false; return }
  // 入力中・修飾キー付き・モーダル表示中はショートカットを効かせない。
  if (event.ctrlKey || event.metaKey || event.altKey || isTyping(event.target) || document.querySelector('dialog[open]')) return
  const match = VIEWS.find((option) => option.shortcut === event.key.toUpperCase())
  if (match) choose(match.value)
}

function handleOutsideClick(event: MouseEvent) {
  if (open.value && !root.value?.contains(event.target as Node)) open.value = false
}

onMounted(() => { window.addEventListener('keydown', handleKeydown); document.addEventListener('click', handleOutsideClick) })
onBeforeUnmount(() => { window.removeEventListener('keydown', handleKeydown); document.removeEventListener('click', handleOutsideClick) })
</script>

<template>
  <div ref="root" class="view-menu">
    <button type="button" class="trigger" aria-haspopup="menu" :aria-expanded="open" aria-label="表示の切り替え" @click="open = !open">
      <span>{{ VIEWS.find((option) => option.value === view)?.label }}</span>
      <AppIcon name="arrow_drop_down" />
    </button>
    <ul v-if="open" class="menu" role="menu">
      <li v-for="option in VIEWS" :key="option.value" role="none">
        <button type="button" role="menuitemradio" :aria-checked="view === option.value" :class="{ active: view === option.value }" @click="choose(option.value)">
          <span>{{ option.label }}</span>
          <kbd>{{ option.shortcut }}</kbd>
        </button>
      </li>
    </ul>
  </div>
</template>

<style scoped>
.view-menu { position: relative; }
.trigger { display: flex; align-items: center; justify-content: space-between; gap: 4px; width: 100%; min-width: 72px; min-height: 40px; padding: 0 6px 0 14px; border: 1px solid var(--field); border-radius: 4px; background: transparent; color: var(--text-high); font-size: 14px; font-weight: 600; }
.trigger:hover { background: rgba(255,255,255,.04); }
.menu { position: absolute; right: 0; top: calc(100% + 4px); z-index: 10; min-width: 160px; margin: 0; padding: 4px 0; list-style: none; border-radius: 4px; background: var(--surface-2); box-shadow: 0 8px 24px rgba(0,0,0,.5); }
.menu button { display: flex; align-items: center; justify-content: space-between; width: 100%; padding: 10px 16px; border: 0; background: transparent; color: var(--text-high); font-size: 15px; text-align: left; }
.menu button:hover { background: rgba(255,255,255,.06); }
.menu button.active { background: rgba(255,255,255,.1); }
.menu kbd { font-family: inherit; font-size: 12px; font-weight: 700; color: var(--text-medium); }
</style>
