<script lang="ts">
	// ─────────────────────────────────────────────────────────────────────────
	// タスク タブ（旧 app.js の tasks 関連 + index.html #tab-tasks を移植）。
	// 参照実装（§11.3）: 共通 Modal 機構 / 再帰 SubtaskRow / chart.js 動的 import。
	//
	// 挙動の忠実移植:
	//   - 表示モード切替（一覧 / ガント）: ローカル state（旧 data-task-view）。
	//   - フィルタ（all/pending/done）: ローカル state（旧 data-filter）。
	//   - いつかやる（someday）は一覧モードで別 fetch・0件時は非表示。
	//   - 完了トグルは check→complete / uncheck→update(status:open)（旧 toggleTaskCompletion）。
	//   - 削除は ConfirmDialog（旧 confirm）で確認。
	//   - activeBot 変更でリロード（bot-scoped API）。
	// ─────────────────────────────────────────────────────────────────────────
	import { activeBot } from "$lib/stores/activeBot";
	import { taskApi } from "$lib/api/services";
	import { ApiError } from "$lib/api/client";
	import { pushToast } from "$lib/stores/toast";
	import { confirmDialog } from "$lib/components/ui";
	import { Button, Icon, EmptyState } from "$lib/components/ui";
	import type { TodoWithSubtasks, TodoPriority } from "$lib/api/types";

	import TaskCard from "./tasks/TaskCard.svelte";
	import TaskEditModal from "./tasks/TaskEditModal.svelte";
	import SubtaskModal from "./tasks/SubtaskModal.svelte";
	import TaskProgressModal from "./tasks/TaskProgressModal.svelte";
	import GanttChart from "./tasks/GanttChart.svelte";

	type ViewMode = "list" | "gantt";
	type Filter = "all" | "pending" | "done";

	let viewMode = $state<ViewMode>("list");
	let filter = $state<Filter>("all");

	let tasks = $state<TodoWithSubtasks[]>([]);
	let somedayTasks = $state<TodoWithSubtasks[]>([]);
	let ganttTasks = $state<TodoWithSubtasks[]>([]);
	let loading = $state(false);

	// モーダル状態
	let editOpen = $state(false);
	let editingTask = $state<TodoWithSubtasks | null>(null);
	let subOpen = $state(false);
	let subParent = $state<TodoWithSubtasks | null>(null);
	let progressOpen = $state(false);
	let progressTask = $state<TodoWithSubtasks | null>(null);

	function reportError(e: unknown) {
		const msg = e instanceof ApiError ? e.message : "エラーが発生しました";
		pushToast(msg, "error");
	}

	// ── 一覧＋いつかやるを取得（旧 fetchTasksList → fetchSomedayTasks） ──
	async function loadList() {
		loading = true;
		try {
			const res = await taskApi.list({ status: filter });
			tasks = res.tasks ?? [];
		} catch (e) {
			reportError(e);
			tasks = [];
		} finally {
			loading = false;
		}
		try {
			const res = await taskApi.someday();
			somedayTasks = res.tasks ?? [];
		} catch (e) {
			reportError(e);
			somedayTasks = [];
		}
	}

	// ── ガント取得（旧 renderGantt の fetch 部分） ──
	async function loadGantt() {
		try {
			const res = await taskApi.gantt();
			ganttTasks = res.tasks ?? [];
		} catch (e) {
			reportError(e);
			ganttTasks = [];
		}
	}

	// activeBot（bot-scoped）変更 or viewMode/filter 変更で再取得。
	// $effect 内で activeBot を購読することで Bot 切替に追従する。
	$effect(() => {
		// 依存を明示的に読む。
		void $activeBot?.id;
		if (viewMode === "list") {
			void filter; // filter 変更でも再取得
			void loadList();
		} else {
			void loadGantt();
		}
	});

	async function reloadCurrent() {
		if (viewMode === "list") await loadList();
		else await loadGantt();
	}

	// ── 完了トグル（旧 toggleTaskCompletion） ──
	async function onToggle(id: number, checked: boolean) {
		try {
			if (checked) await taskApi.complete(id);
			else await taskApi.update({ id, status: "open" });
			await reloadCurrent();
		} catch (e) {
			reportError(e);
		}
	}

	// ── 削除（旧 handleDeleteTask、confirm → ConfirmDialog） ──
	async function onDelete(id: number) {
		const ok = await confirmDialog({
			message:
				"本当にこのタスクを削除しますか？（サブタスクや進捗履歴も一緒に削除されます）",
			danger: true,
			confirmLabel: "削除",
		});
		if (!ok) return;
		try {
			await taskApi.delete(id);
			await reloadCurrent();
		} catch (e) {
			reportError(e);
		}
	}

	// ── モーダルを開く ──
	function openNewTask() {
		editingTask = null;
		editOpen = true;
	}
	function onEdit(task: TodoWithSubtasks) {
		editingTask = task;
		editOpen = true;
	}
	function onAddSub(parent: TodoWithSubtasks) {
		subParent = parent;
		subOpen = true;
	}
	function onProgress(task: TodoWithSubtasks) {
		progressTask = task;
		progressOpen = true;
	}

	// ── モーダル保存ハンドラ ──
	async function saveTask(payload: {
		id: number | null;
		title: string;
		description: string;
		startDate: string;
		dueDate: string;
		priority: TodoPriority | null;
	}) {
		try {
			if (payload.id != null) {
				await taskApi.update({
					id: payload.id,
					title: payload.title,
					description: payload.description,
					startDate: payload.startDate,
					dueDate: payload.dueDate,
					priority: payload.priority,
				});
			} else {
				await taskApi.add({
					title: payload.title,
					description: payload.description,
					startDate: payload.startDate,
					dueDate: payload.dueDate,
					priority: payload.priority ?? undefined,
				});
			}
			editOpen = false;
			await reloadCurrent();
		} catch (e) {
			reportError(e);
		}
	}

	async function saveSubtask(payload: {
		parentId: number;
		title: string;
		startDate: string;
		dueDate: string;
	}) {
		try {
			await taskApi.add({
				title: payload.title,
				startDate: payload.startDate,
				dueDate: payload.dueDate,
				parentId: payload.parentId,
			});
			subOpen = false;
			await reloadCurrent();
		} catch (e) {
			reportError(e);
		}
	}

	async function saveProgress(payload: {
		id: number;
		progress: number;
		note: string;
	}) {
		try {
			await taskApi.progress(payload);
			progressOpen = false;
			await reloadCurrent();
		} catch (e) {
			reportError(e);
		}
	}
</script>

<section class="tab-view">
	<!-- 上部アクション: 表示モード切替 + 使い方リンク + タスク追加 -->
	<div class="view-actions-card card">
		<div class="filters-group">
			<button
				type="button"
				class="btn btn-filter"
				class:active={viewMode === "list"}
				onclick={() => (viewMode = "list")}><Icon name="view_list" size={16} /> 一覧</button
			>
			<button
				type="button"
				class="btn btn-filter"
				class:active={viewMode === "gantt"}
				onclick={() => (viewMode = "gantt")}><Icon name="bar_chart" size={16} /> ガント</button
			>
		</div>
		<div class="tasks-actions-right">
			<a
				href="/admin/tasks/guide"
				class="btn btn-secondary tasks-guide-link"
			>
				<Icon name="help" size={18} /> 使い方
			</a>
			<Button variant="primary" onclick={openNewTask}>＋ タスク追加</Button>
		</div>
	</div>

	{#if viewMode === "list"}
		<div>
			<!-- フィルタ（all/pending/done） -->
			<div class="view-actions-card card">
				<div class="filters-group">
					<button
						type="button"
						class="btn btn-filter"
						class:active={filter === "all"}
						onclick={() => (filter = "all")}>全て</button
					>
					<button
						type="button"
						class="btn btn-filter"
						class:active={filter === "pending"}
						onclick={() => (filter = "pending")}>未完了</button
					>
					<button
						type="button"
						class="btn btn-filter"
						class:active={filter === "done"}
						onclick={() => (filter = "done")}>完了済み</button
					>
				</div>
			</div>

			<div class="list-container">
				{#if tasks.length > 0}
					{#each tasks as task (task.id)}
						<TaskCard
							{task}
							{onToggle}
							{onEdit}
							{onAddSub}
							{onProgress}
							{onDelete}
						/>
					{/each}
				{:else if !loading}
					<EmptyState icon="task_alt" message="登録されているタスクがありません。" />
				{/if}
			</div>

			<!-- いつかやる（日付未設定タスク）。0件時は非表示（旧仕様）。 -->
			{#if somedayTasks.length > 0}
				<div class="tasks-someday-section">
					<h3 class="tasks-someday-heading"><Icon name="schedule" size={18} /> いつかやる（日付未設定）</h3>
					<div class="list-container">
						{#each somedayTasks as task (task.id)}
							<TaskCard
								{task}
								{onToggle}
								{onEdit}
								{onAddSub}
								{onProgress}
								{onDelete}
							/>
						{/each}
					</div>
				</div>
			{/if}
		</div>
	{:else}
		<GanttChart tasks={ganttTasks} />
	{/if}
</section>

<!-- モーダル群（ストア駆動 Modal でラップ） -->
<TaskEditModal bind:open={editOpen} {editingTask} onsave={saveTask} />
<SubtaskModal bind:open={subOpen} parent={subParent} onsave={saveSubtask} />
<TaskProgressModal bind:open={progressOpen} task={progressTask} onsave={saveProgress} />

<style>
	.tasks-actions-right {
		display: flex;
		gap: 8px;
	}
	.tasks-guide-link {
		display: inline-flex;
		align-items: center;
		gap: 6px;
	}
	.tasks-someday-section {
		margin-top: 1.5rem;
	}
	.tasks-someday-heading {
		font-size: 0.95rem;
		opacity: 0.8;
		margin-bottom: 0.5rem;
	}
</style>
