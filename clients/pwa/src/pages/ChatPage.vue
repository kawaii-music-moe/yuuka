<script setup lang="ts">
import { nextTick, onBeforeUnmount, onMounted, ref } from 'vue'
import { agentGateway, ApiError, ChatReplyTimeoutError, type ChatMessage } from '@/api'
import AppIcon from '@/components/AppIcon.vue'
import ChatRichContent from '@/components/ChatRichContent.vue'
import ChatComposer from '@/components/chat/ChatComposer.vue'
import MarkdownContent from '@/components/MarkdownContent.vue'
import PageState from '@/components/PageState.vue'
import ReferenceCard from '@/components/ReferenceCard.vue'
import { prepareChatAttachment } from '@/utils/chatAttachments'
import { time } from '@/utils/format'
/** この時間を超えても応答が届かなければ「時間がかかっています」を表示する。 */
const SLOW_HINT_AFTER_MS = 20_000
const messages = ref<ChatMessage[]>([]); const composer = ref<InstanceType<typeof ChatComposer>>(); const loading = ref(true); const sending = ref(false); const slow = ref(false); const error = ref(''); const thread = ref<HTMLElement>()
// 応答待ち（ポーリング）を、画面を離れたときに止めるための AbortController。
let pending: AbortController | undefined
async function scrollToLatest() { await nextTick(); thread.value?.scrollTo({ top: thread.value.scrollHeight, behavior: 'smooth' }) }
function describeError(cause: unknown): string {
  // 同期的な事前チェックの拒否（キー未設定 400 / 処理中 409 / レート制限 429）はサーバーの案内文をそのまま出す。
  if (cause instanceof ApiError && cause.serverMessage && [400, 409, 429].includes(cause.status ?? 0)) return cause.serverMessage
  if (cause instanceof ChatReplyTimeoutError) return 'エージェントの応答を待ちきれませんでした。しばらくしてから、画面を開き直して履歴を確認してください。'
  if (cause instanceof ApiError && cause.status === 401) return 'ログインの有効期限が切れました。ログインし直してください。'
  return 'メッセージを送信できませんでした。'
}
/** 応答待ちの共通処理（送信直後・リロード後の再開の両方）。待機中は入力を無効化しタイピング表示を出す。 */
async function awaitReply(wait: (options: { signal: AbortSignal; onTick: (info: { elapsedMs: number }) => void }) => Promise<ChatMessage>) {
  pending?.abort(); const controller = new AbortController(); pending = controller
  sending.value = true; slow.value = false
  try {
    const reply = await wait({ signal: controller.signal, onTick: ({ elapsedMs }) => { slow.value = elapsedMs >= SLOW_HINT_AFTER_MS } })
    messages.value.push(reply)
  } catch (cause) {
    if (controller.signal.aborted) return
    error.value = describeError(cause)
    throw cause
  } finally {
    if (pending === controller) { sending.value = false; slow.value = false; pending = undefined; await scrollToLatest() }
  }
}
async function load() {
  try {
    messages.value = await agentGateway.listChatMessages(); await scrollToLatest()
    // 直近が自分の発言のままなら、応答待ち（サーバーはバックグラウンドで生成中）の可能性がある。リロードしても待ちを再開する。
    const last = messages.value.at(-1)
    if (last?.role === 'user') awaitReply((options) => agentGateway.waitForChatReply(last.id, options)).catch(() => undefined)
  } catch { error.value = 'チャット履歴を取得できません。' } finally { loading.value = false }
}
async function send({ content, files }: { content: string; files: File[] }) {
  if (sending.value) return
  error.value = ''
  // 送信中の自分の発言は、添付をこの端末のファイルのまま（object URL で）表示しておく。
  const localFiles = files.map((file, index) => ({ id: `local-${index}`, name: file.name, mimeType: file.type, url: URL.createObjectURL(file) }))
  const local: ChatMessage = { id: `local-${Date.now()}`, role: 'user', content, createdAt: new Date().toISOString(), files: localFiles }
  messages.value.push(local); await scrollToLatest()
  try {
    const attachments = await Promise.all(files.map(prepareChatAttachment))
    await awaitReply((options) => agentGateway.sendChatMessage(content, attachments, options))
  } catch (cause) {
    // 同期的に拒否された（サーバーに保存されていない）発言は取り消して入力欄へ戻し、そのまま再送できるようにする。
    if (!(cause instanceof ApiError) || [400, 409, 413, 429].includes(cause.status ?? 0)) {
      messages.value = messages.value.filter((message) => message !== local)
      localFiles.forEach((file) => URL.revokeObjectURL(file.url))
      composer.value?.restore(content, files)
      if (!(cause instanceof ApiError)) error.value = '添付ファイルを読み込めませんでした。'
      else if (cause.status === 413) error.value = '添付ファイルが大きすぎます。'
    }
  }
}
onMounted(load)
onBeforeUnmount(() => pending?.abort())
</script>
<template><section class="chat-page"><div class="chat-intro"><RouterLink to="/notes" class="text-link">共有ノート</RouterLink></div><PageState :loading="loading" :error="error"/><div v-if="!loading" ref="thread" class="thread"><article v-for="message in messages" :key="message.id" class="message" :class="message.role"><div class="avatar"><AppIcon :name="message.role === 'agent' ? 'smart_toy' : 'person'" /></div><div class="message-body"><div class="message-meta"><strong>{{ message.role === 'agent' ? 'Agent Desk' : 'あなた' }}</strong><time>{{ time(message.createdAt) }}</time></div><MarkdownContent :source="message.content" /><ChatRichContent :embeds="message.embeds" :files="message.files" /><ReferenceCard v-for="reference in message.references" :key="`${message.id}-${reference.href}`" :reference="reference" /></div></article><div v-if="sending" class="typing" role="status"><span></span><span></span><span></span> エージェントが入力中<em v-if="slow">（時間がかかっています。しばらくお待ちください）</em></div></div><ChatComposer ref="composer" :disabled="sending" @send="send" /></section></template>
<style scoped>
.chat-page{max-width:820px;margin:0 auto}.chat-intro{display:flex;justify-content:flex-end;align-items:end;border-bottom:1px solid var(--line);padding-bottom:15px}.text-link{font-size:13px;color:var(--primary);white-space:nowrap}.thread{min-height:calc(100dvh - 194px);max-height:calc(100dvh - 194px);overflow-y:auto;padding:18px 2px}.message{display:flex;gap:10px;margin-bottom:18px;max-width:92%}.message.user{margin-left:auto;flex-direction:row-reverse}.avatar{width:30px;height:30px;display:grid;place-items:center;color:var(--primary);background:var(--primary-soft);border:1px solid var(--primary-border);border-radius:2px;flex:none}.user .avatar{color:var(--text-medium);background:var(--surface-2);border-color:var(--line)}.message-body{min-width:0;flex:1}.message-meta{display:flex;gap:8px;align-items:center;margin-bottom:5px}.message-meta strong{font-size:12px}.message-meta time{font-size:11px;color:var(--text-medium)}.user .message-meta{justify-content:end}.user .message-body{background:var(--primary-soft);border-left:3px solid var(--primary);padding:10px 12px}.agent .message-body{border-left:2px solid var(--line);padding:3px 0 3px 12px}.typing{font-size:12px;color:var(--text-medium);display:flex;gap:4px;align-items:center}.typing em{font-style:normal;margin-left:4px}.typing span{width:5px;height:5px;background:var(--text-medium);border-radius:50%}@media(max-width:640px){.chat-page{margin:0 -16px}.chat-intro{padding:0 16px 12px}.thread{padding:16px;min-height:calc(100dvh - 128px);max-height:calc(100dvh - 128px)}.message{max-width:100%;margin-bottom:15px}.chat-page>.composer{position:sticky;bottom:8px;margin:8px 12px 0}}</style>
