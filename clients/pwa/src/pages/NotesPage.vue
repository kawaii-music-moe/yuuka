<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { agentGateway } from '@/api'
import { useAsyncState } from '@/composables/useAsyncState'
import { useSavedNotice } from '@/composables/useSavedNotice'
import FormField from '@/components/FormField.vue'
import PageState from '@/components/PageState.vue'
import PageTitle from '@/components/PageTitle.vue'
import UiButton from '@/components/UiButton.vue'

const note = ref<{ title: string; body: string }>()
const { loading, error, run } = useAsyncState()
const { visible: saved, show: showSaved } = useSavedNotice()
const saveError = ref('')

async function load() {
  await run(async () => {
    const result = await agentGateway.getSharedNote()
    note.value = { title: result.title, body: result.body }
  }, '共有ノートを取得できません。')
}

async function save() {
  if (!note.value) return
  saveError.value = ''
  try {
    const result = await agentGateway.saveSharedNote(note.value)
    note.value = { title: result.title, body: result.body }
    showSaved()
  } catch {
    saveError.value = '共有ノートを保存できませんでした。もう一度お試しください。'
  }
}

onMounted(load)
</script>

<template>
  <section>
    <PageTitle title="共有ノート" description="エージェントと共有する長期メモです">
      <template #actions><UiButton :disabled="!note" @click="save">保存</UiButton></template>
    </PageTitle>
    <PageState :loading="loading" :error="error" />
    <form v-if="note" class="note-form" @submit.prevent="save">
      <FormField label="タイトル"><input v-model="note.title" /></FormField>
      <FormField label="本文（Markdown）"><textarea v-model="note.body" spellcheck="false" /></FormField>
      <p v-if="saved" class="saved">保存しました。</p>
      <p v-if="saveError" class="error save-error">{{ saveError }}</p>
    </form>
  </section>
</template>

<style scoped>
.note-form { max-width: 850px; display: grid; gap: 16px; }
.note-form textarea { min-height: 430px; font-family: ui-monospace, SFMono-Regular, Consolas, monospace; font-size: 13px; }
.saved { color: var(--primary); font-size: 13px; margin: 0; }
.save-error { font-size: 13px; margin: 0; }
</style>
