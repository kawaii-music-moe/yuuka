<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { AppIcon, PageState, UiButton } from '@/components/ui'
import { selectClientBot } from '@/stores/botSelection'
import AdminPageShell from '../components/AdminPageShell.vue'
import { listAdminBots, type AdminBot } from '../api/bots'

const router = useRouter()
const bots = ref<AdminBot[]>([])
const loading = ref(true)
const error = ref('')
const active = computed(() => bots.value.filter((bot) => !bot.suspended && bot.running).length)
const stopped = computed(() => bots.value.filter((bot) => !bot.running || bot.suspended).length)
async function load(): Promise<void> { loading.value = true; error.value = ''; try { bots.value = await listAdminBots() } catch { error.value = 'Bot一覧を読み込めませんでした。' } finally { loading.value = false } }
function openClient(bot: AdminBot): void { selectClientBot(bot); void router.push({ path: '/', query: { botId: bot.id } }) }
onMounted(load)
</script>

<template>
  <AdminPageShell title="Bot一覧">
    <template #actions><UiButton variant="secondary" icon="refresh" aria-label="更新" @click="load" /></template>
    <section class="admin-bot-overview"><div><h2>アシスタントを選択</h2><p>選択したBotのClientへ移動します。</p></div><dl class="bot-stats"><div><dt>登録済み</dt><dd>{{ bots.length }}</dd></div><div><dt>稼働中</dt><dd>{{ active }}</dd></div><div><dt>停止中</dt><dd>{{ stopped }}</dd></div></dl></section>
    <PageState :loading="loading" :error="error" />
    <div v-if="!loading" class="bot-list"><button v-for="bot in bots" :key="bot.id" class="bot-row" type="button" @click="openClient(bot)"><span class="bot-row__avatar"><img v-if="bot.avatarUrl" :src="bot.avatarUrl" alt="" /><AppIcon v-else name="smart_toy" /></span><span class="bot-row__body"><strong>{{ bot.name }}</strong><small>{{ bot.preset_display_name || bot.preset || '標準設定' }}</small></span><span class="bot-row__state" :class="{ stopped: !bot.running || bot.suspended }">{{ bot.connected ? '接続中' : bot.running ? '起動中' : '停止中' }}</span><AppIcon name="chevron_right" /></button><p v-if="bots.length === 0" class="state-text">利用できるBotはありません。</p></div>
  </AdminPageShell>
</template>

<style scoped>
.admin-bot-overview{display:flex;justify-content:space-between;gap:24px;padding-bottom:24px;border-bottom:1px solid var(--line)}.admin-bot-overview h2{margin:0;font-size:22px}.admin-bot-overview p{margin:6px 0 0;color:var(--text-medium);font-size:13px}.bot-stats{display:flex;margin:0;border:1px solid var(--line)}.bot-stats div{min-width:84px;padding:10px 14px;border-right:1px solid var(--line)}.bot-stats div:last-child{border:0}.bot-stats dt{color:var(--text-medium);font-size:11px}.bot-stats dd{margin:3px 0 0;font-size:20px;font-weight:700}.bot-list{border-top:1px solid var(--line)}.bot-row{width:100%;min-height:78px;display:flex;align-items:center;gap:13px;padding:12px 4px;border:0;border-bottom:1px solid var(--line);background:transparent;color:var(--text-high);text-align:left}.bot-row:hover{background:var(--primary-soft)}.bot-row__avatar{width:42px;height:42px;display:grid;place-items:center;overflow:hidden;border:1px solid var(--primary-border);border-radius:4px;color:var(--primary);background:var(--surface-2)}.bot-row__avatar img{width:100%;height:100%;object-fit:cover}.bot-row__body{display:grid;gap:4px;min-width:0;flex:1}.bot-row__body strong{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:14px}.bot-row__body small{color:var(--text-medium);font-size:12px}.bot-row__state{color:var(--primary);font-size:12px}.bot-row__state.stopped{color:var(--text-medium)}@media(max-width:640px){.admin-bot-overview{display:grid}.bot-stats{width:100%}.bot-stats div{flex:1;min-width:0}.bot-row__state{display:none}}
</style>
