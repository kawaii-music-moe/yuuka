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

async function load() {
  await run(async () => {
    const result = await agentGateway.getSharedNote()
    note.value = { title: result.title, body: result.body }
  }, 'Shared note could not be loaded.')
}

async function save() {
  if (!note.value) return
  await agentGateway.saveSharedNote(note.value)
  showSaved()
}

onMounted(load)
</script>

<template>
  <section>
    <PageTitle title="Shared note" description="Persistent memory shared with the agent.">
      <template #actions><UiButton :disabled="!note" @click="save">Save</UiButton></template>
    </PageTitle>
    <PageState :loading="loading" :error="error" />
    <form v-if="note" class="note-form" @submit.prevent="save">
      <p class="notice">Changes are saved to the agent's shared memory.</p>
      <FormField label="Title"><input v-model="note.title" /></FormField>
      <FormField label="Markdown"><textarea v-model="note.body" spellcheck="false" /></FormField>
      <p v-if="saved" class="saved">Saved.</p>
    </form>
  </section>
</template>

<style scoped>
.note-form { max-width: 850px; display: grid; gap: 16px; }
.notice { font-size: 13px; color: #475467; border-left: 3px solid var(--primary); padding: 9px 12px; margin: 0; background: var(--primary-soft); }
.note-form textarea { min-height: 430px; font-family: ui-monospace, SFMono-Regular, Consolas, monospace; font-size: 13px; }
.saved { color: var(--primary); font-size: 13px; margin: 0; }
</style>
