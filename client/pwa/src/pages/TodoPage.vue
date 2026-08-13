<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { agentGateway, type Todo } from '@/api'
import PageState from '@/components/PageState.vue'
import AppIcon from '@/components/AppIcon.vue'
import PageTitle from '@/components/PageTitle.vue'
import UiButton from '@/components/UiButton.vue'
import { date } from '@/utils/format'
const todos = ref<Todo[]>([]); const title = ref(''); const dueDate = ref(''); const list = ref('個人'); const loading = ref(true); const error = ref(''); const showDone = ref(false)
const visibleTodos = computed(() => todos.value.filter(x => showDone.value || !x.completed))
async function load() { try { todos.value = await agentGateway.listTodos() } catch { error.value = 'タスクを取得できません。' } finally { loading.value = false } }
async function add() { if (!title.value.trim()) return; const item = await agentGateway.createTodo({ title: title.value, dueDate: dueDate.value || undefined, list: list.value }); todos.value.unshift(item); title.value = ''; dueDate.value = '' }
async function toggle(todo: Todo) { const updated = await agentGateway.updateTodo(todo.id, { completed: !todo.completed }); Object.assign(todo, updated) }
onMounted(load)
</script>
<template><section><PageTitle title="タスク" description="やることを整理して、完了まで追跡します"/><form class="add-task" @submit.prevent="add"><input v-model="title" placeholder="タスクを追加" aria-label="タスク名" /><input v-model="dueDate" type="date" aria-label="期限" /><select v-model="list" aria-label="リスト"><option>個人</option><option>仕事</option></select><UiButton icon="add" type="submit" aria-label="追加" /></form><div class="toolbar"><span>{{ todos.filter(x => !x.completed).length }} 件の未完了</span><label><input v-model="showDone" type="checkbox" /> 完了済みを表示</label></div><PageState :loading="loading" :error="error" /><div v-if="!loading" class="todo-list"><article v-for="todo in visibleTodos" :key="todo.id" class="todo-row" :class="{done: todo.completed}"><button class="check-button" :aria-label="todo.completed ? '未完了にする' : '完了にする'" @click="toggle(todo)"><AppIcon :name="todo.completed ? 'check_box' : 'check_box_outline_blank'" /></button><div><strong>{{ todo.title }}</strong><p><span>{{ todo.list }}</span><time v-if="todo.dueDate">{{ date(todo.dueDate) }}</time></p></div></article><p v-if="visibleTodos.length === 0" class="state-text">表示するタスクはありません。</p></div></section></template>
<style scoped>.add-task{display:grid;grid-template-columns:1fr 145px 100px 42px;gap:8px;border-bottom:1px solid var(--line);padding-bottom:16px}.add-task input,.add-task select{border:1px solid var(--field);border-radius:2px;padding:8px}.add-task button{padding:0;display:grid;place-items:center}.toolbar{display:flex;justify-content:space-between;padding:14px 0;font-size:13px;color:#475467}.todo-list{border-top:1px solid var(--line)}.todo-row{display:flex;gap:12px;padding:15px 4px;border-bottom:1px solid var(--line)}.check-button{border:0;background:transparent;padding:0;color:var(--primary);align-self:start}.todo-row strong{font-size:14px}.todo-row p{font-size:12px;color:#667085;margin:5px 0 0;display:flex;gap:10px}.todo-row p span{color:var(--primary)}.todo-row.done strong{text-decoration:line-through;color:#667085}@media(max-width:640px){.add-task{grid-template-columns:1fr 42px}.add-task input[type=date],.add-task select{grid-row:2}.toolbar{gap:8px;align-items:center}}</style>
