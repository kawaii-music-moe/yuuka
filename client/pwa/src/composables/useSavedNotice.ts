import { onBeforeUnmount, ref } from 'vue'

export function useSavedNotice(duration = 2500) {
  const visible = ref(false)
  let timer: ReturnType<typeof setTimeout> | undefined

  function show() {
    visible.value = true
    if (timer) clearTimeout(timer)
    timer = setTimeout(() => { visible.value = false }, duration)
  }

  onBeforeUnmount(() => { if (timer) clearTimeout(timer) })
  return { visible, show }
}
