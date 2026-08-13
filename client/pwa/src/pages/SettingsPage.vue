<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { agentGateway, type AgentSettings } from '@/api'
import PageState from '@/components/PageState.vue'
import FormField from '@/components/FormField.vue'
import PageTitle from '@/components/PageTitle.vue'
import UiButton from '@/components/UiButton.vue'
import ContentSection from '@/components/ContentSection.vue'
const settings = ref<AgentSettings>(); const loading = ref(true); const error = ref(''); const saved = ref(false)
onMounted(async () => { try { settings.value = await agentGateway.getSettings() } catch { error.value = '設定を取得できません。' } finally { loading.value = false } })
async function save() { if (!settings.value) return; settings.value = await agentGateway.saveSettings(settings.value); saved.value = true; setTimeout(() => saved.value = false, 2500) }
async function authorize() { const result = await agentGateway.startGoogleAuthorization(); window.open(result.authorizationUrl, '_blank', 'noopener,noreferrer') }
</script>
<template><section><PageTitle title="設定" description="エージェントの連携と応答方針を管理します"><template #actions><UiButton :disabled="!settings" @click="save">保存</UiButton></template></PageTitle><PageState :loading="loading" :error="error"/><form v-if="settings" class="settings-form" @submit.prevent="save"><p v-if="saved" class="saved">保存しました。</p><ContentSection title="Google 連携"><div class="integration"><div><strong>{{ settings.googleConnected ? '接続済み' : '未接続' }}</strong><p>{{ settings.googleAccount ?? 'カレンダーとタスクの連携を有効にします。' }}</p></div><UiButton variant="secondary" @click="authorize">{{ settings.googleConnected ? '再認証' : '連携する' }}</UiButton></div></ContentSection><ContentSection title="AI 応答"><div class="field-grid"><FormField label="使用モデル"><select v-model="settings.model"><option>GPT-4o</option><option>Claude</option><option>GPT-4.1</option></select></FormField><FormField :label="`Max Tokens: ${settings.maxTokens}`"><input v-model.number="settings.maxTokens" type="range" min="256" max="8192" step="256" /></FormField><FormField :label="`Temperature: ${settings.temperature}`"><input v-model.number="settings.temperature" type="range" min="0" max="2" step="0.1" /></FormField></div></ContentSection><ContentSection title="ペルソナ"><FormField label="エージェントの口調・振る舞い"><textarea v-model="settings.persona" /></FormField></ContentSection></form></section></template>
<style scoped>.settings-form{max-width:760px}.integration{display:flex;justify-content:space-between;align-items:center;background:var(--primary-soft);border-left:3px solid var(--primary);padding:13px}.integration strong{font-size:14px}.integration p{font-size:12px;color:var(--text-medium);margin:4px 0 0}.field-grid{display:grid;grid-template-columns:repeat(3,1fr);gap:14px}.saved{color:var(--primary);font-size:13px;margin:0 0 14px}@media(max-width:640px){.field-grid{grid-template-columns:1fr}.integration{gap:12px}.integration .ui-button{flex-shrink:0}}</style>
