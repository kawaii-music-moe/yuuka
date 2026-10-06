<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { agentGateway, type Todo } from '@/api'
import PageState from '@/components/PageState.vue'
import AppIcon from '@/components/AppIcon.vue'
import PageTitle from '@/components/PageTitle.vue'
import { date } from '@/utils/format'
const todos = ref<Todo[]>([]); const loading = ref(true); const error = ref('')
onMounted(async () => { try { todos.value = await agentGateway.listTodos() } catch { error.value = 'エージェントサーバーに接続できません。' } finally { loading.value = false } })
</script>
<template>
  <section class="dashboard"><PageTitle title="おはようございます" description="今日の状況を確認しましょう" />
  <PageState :loading="loading" :error="error" />
  <template v-if="!loading && !error">
  <div class="dashboard-grid"><section><h3 class="section-title">未完了のタスク</h3><div class="surface task-list"><RouterLink v-for="todo in todos.filter(x => !x.completed).slice(0, 4)" :key="todo.id" to="/todo" class="task-row"><span class="checkbox"></span><span>{{ todo.title }}</span><time v-if="todo.dueDate">{{ date(todo.dueDate) }}</time></RouterLink><RouterLink to="/todo" class="list-footer">すべてのタスクを見る</RouterLink></div></section>
  <section><h3 class="section-title">すぐ使う</h3><div class="quick-links"><RouterLink to="/calendar"><AppIcon name="calendar_month" />予定を確認</RouterLink><RouterLink to="/finance"><AppIcon name="account_balance_wallet" />支出を記録</RouterLink><RouterLink to="/notes"><AppIcon name="description" />共有ノート</RouterLink></div></section></div></template></section>
</template>
<style scoped>.dashboard-grid{display:grid;grid-template-columns:1.3fr 1fr;gap:28px}.task-row{min-height:52px;display:flex;align-items:center;gap:11px;padding:0 14px;border-bottom:1px solid var(--line);font-size:14px}.task-row time{margin-left:auto;color:var(--text-medium);font-size:12px}.checkbox{width:18px;height:18px;border:1px solid var(--text-medium);border-radius:2px}.list-footer{display:block;padding:12px 14px;color:var(--primary);font-size:13px}.quick-links{border-top:1px solid var(--line)}.quick-links a{padding:14px 4px;border-bottom:1px solid var(--line);display:flex;gap:10px;align-items:center;font-size:14px}.quick-links .material-symbols-outlined{color:var(--primary)}@media(max-width:640px){.dashboard-grid{grid-template-columns:1fr;gap:24px}}</style>
