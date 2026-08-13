<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { agentGateway } from '@/api'
import PageState from '@/components/PageState.vue'
import FormField from '@/components/FormField.vue'
import PageTitle from '@/components/PageTitle.vue'
import UiButton from '@/components/UiButton.vue'
const note = ref<{ title: string; body: string }>(); const loading = ref(true); const error = ref(''); const saved = ref(false)
onMounted(async () => { try { const result = await agentGateway.getSharedNote(); note.value = { title: result.title, body: result.body } } catch { error.value = '共有ノートを取得できません。' } finally { loading.value = false } })
async function save() { if (!note.value) return; await agentGateway.saveSharedNote(note.value); saved.value = true; setTimeout(() => saved.value = false, 2500) }
</script>
<template><section><PageTitle title="共有ノート" description="エージェントと共有する永続メモリ"><template #actions><UiButton :disabled="!note" @click="save">保存</UiButton></template></PageTitle><PageState :loading="loading" :error="error"/><form v-if="note" class="note-form" @submit.prevent="save"><p class="notice">ここで保存した内容は、エージェントが参照する共有コンテキストとして扱います。</p><FormField label="タイトル"><input v-model="note.title" /></FormField><FormField label="Markdown"><textarea v-model="note.body" spellcheck="false" /></FormField><p v-if="saved" class="saved">保存しました。</p></form></section></template>
<style scoped>.note-form{max-width:850px;display:grid;gap:16px}.notice{font-size:13px;color:#475467;border-left:3px solid var(--primary);padding:9px 12px;margin:0;background:var(--primary-soft)}.note-form textarea{min-height:430px;font-family:ui-monospace,SFMono-Regular,Consolas,monospace;font-size:13px}.saved{color:var(--primary);font-size:13px;margin:0}</style>
