<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { agentGateway, type Todo } from '@/api'
import PageState from '@/components/PageState.vue'
import AppIcon from '@/components/AppIcon.vue'
import PageTitle from '@/components/PageTitle.vue'
import UiButton from '@/components/UiButton.vue'
import AlertModal from '@/components/AlertModal.vue'
import { date } from '@/utils/format'
const todos = ref<Todo[]>([]); const title = ref(''); const dueDate = ref(''); const list = ref('個人'); const loading = ref(true); const error = ref(''); const showDone = ref(false); const actionError = ref(''); const validationError = ref('')
const visibleTodos = computed(() => todos.value.filter(x => showDone.value || !x.completed))
async function load() { try { todos.value = await agentGateway.listTodos() } catch { error.value = 'タスクを取得できません。' } finally { loading.value = false } }
async function add() {
  if (!title.value.trim()) { validationError.value = 'タスク名を入力してください。'; return }
  actionError.value = ''
  try {
    const item = await agentGateway.createTodo({ title: title.value, dueDate: dueDate.value || undefined, list: list.value })
    todos.value.unshift(item)
    title.value = ''
    dueDate.value = ''
  } catch {
    actionError.value = 'タスクを追加できませんでした。もう一度お試しください。'
  }
}
async function toggle(todo: Todo) {
  actionError.value = ''
  try {
    const updated = await agentGateway.updateTodo(todo.id, { completed: !todo.completed })
    Object.assign(todo, updated)
  } catch {
    actionError.value = 'タスクを更新できませんでした。もう一度お試しください。'
  }
}
onMounted(load)
</script>
<template><section><PageTitle title="タスク" description="やることを整理して、完了まで追跡します"/><form class="add-task" @submit.prevent="add"><input v-model="title" placeholder="タスクを追加" aria-label="タスク名" /><input v-model="dueDate" type="date" aria-label="期限" /><select v-model="list" aria-label="リスト"><option>個人</option><option>仕事</option></select><UiButton icon="add" type="submit" aria-label="追加" /></form><p v-if="actionError" class="error action-error">{{ actionError }}</p><div class="toolbar"><span>{{ todos.filter(x => !x.completed).length }} 件の未完了</span><label><input v-model="showDone" type="checkbox" /> 完了済みを表示</label></div><PageState :loading="loading" :error="error" /><div v-if="!loading" class="todo-list"><article v-for="todo in visibleTodos" :key="todo.id" class="todo-row" :class="{done: todo.completed}"><button class="check-button" :aria-label="todo.completed ? '未完了にする' : '完了にする'" @click="toggle(todo)"><AppIcon :name="todo.completed ? 'check_box' : 'check_box_outline_blank'" /></button><div><strong>{{ todo.title }}</strong><p><span>{{ todo.list }}</span><time v-if="todo.dueDate">{{ date(todo.dueDate) }}</time></p></div></article><p v-if="visibleTodos.length === 0" class="state-text">表示するタスクはありません。</p></div><AlertModal v-model="validationError" /></section></template>
<style scoped>.add-task{display:grid;grid-template-columns:1fr 145px 100px 42px;gap:8px;border-bottom:1px solid var(--line);padding-bottom:16px}.action-error{font-size:12px;margin:10px 0 0}.add-task input,.add-task select{min-width:0;border:1px solid var(--field);border-radius:4px;padding:8px;color:var(--text-high);background:#121212}.add-task input:focus,.add-task select:focus{outline:1px solid var(--primary);border-color:var(--primary)}.add-task button{padding:0;display:grid;place-items:center}.toolbar{display:flex;justify-content:space-between;padding:14px 0;font-size:13px;color:var(--text-medium)}.todo-list{border-top:1px solid var(--line)}.todo-row{display:flex;gap:12px;padding:15px 4px;border-bottom:1px solid var(--line)}.check-button{border:0;background:transparent;padding:0;color:var(--primary);align-self:start}.todo-row strong{font-size:14px}.todo-row p{font-size:12px;color:var(--text-medium);margin:5px 0 0;display:flex;gap:10px}.todo-row p span{color:var(--primary)}.todo-row.done strong{text-decoration:line-through;color:var(--text-medium)}@media(max-width:640px){.add-task{grid-template-columns:1fr 100px 42px}.add-task input:not([type]){grid-column:1/3}.add-task input[type=date]{grid-row:2;grid-column:1}.add-task select{grid-row:2;grid-column:2/4}.toolbar{gap:8px;align-items:center}}</style>
