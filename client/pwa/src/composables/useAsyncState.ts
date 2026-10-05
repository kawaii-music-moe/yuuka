import { ref } from 'vue'

export function useAsyncState() {
  const loading = ref(false)
  const error = ref('')

  async function run(task: () => Promise<void>, message: string) {
    loading.value = true
    error.value = ''
    try {
      await task()
    } catch {
      error.value = message
    } finally {
      loading.value = false
    }
  }

  return { loading, error, run }
}
