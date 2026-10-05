<script lang="ts">
// 親タスク1件のカード（旧 app.js:2386 buildTaskCard）。
// チェックボックス／進捗バー／操作ボタン／折りたたみサブタスクを含む。

import type { TodoWithSubtasks } from "$lib/api/types";
import {
	Checkbox,
	Icon,
	MetaItem,
	ProgressBar,
	TagChip,
} from "$lib/components/ui";
import SubtaskRow from "./SubtaskRow.svelte";
import {
	countDescendants,
	displayPercent,
	fmtTaskDate,
	parseTaskTags,
	priorityLabel,
} from "./taskUtils";

interface Props {
	task: TodoWithSubtasks;
	onToggle: (id: number, checked: boolean) => void;
	onEdit: (task: TodoWithSubtasks) => void;
	onAddSub: (parent: TodoWithSubtasks) => void;
	onProgress: (task: TodoWithSubtasks) => void;
	onDelete: (id: number) => void;
}

let { task, onToggle, onEdit, onAddSub, onProgress, onDelete }: Props =
	$props();

const subtasks = $derived(task.subtasks || []);
const hasSubtasks = $derived(subtasks.length > 0);
const percent = $derived(displayPercent(task));
// 旧仕様: サブタスクを持ち進捗<100% のとき手動完了不可。
const checkboxDisabled = $derived(hasSubtasks && percent < 100);
const tags = $derived(parseTaskTags(task.tags));
const doneCount = $derived(subtasks.filter((s) => s.status === "done").length);

// 折りたたみ状態（旧 subtask-toggle。既定は展開＝旧 flex 相当）。
let expanded = $state(true);
</script>

<div class="card-item glass hover-lift task-card-col" class:done={task.status === "done"}>
	<!-- 上段: チェックボックス＋本文＋削除 -->
	<div class="task-card-top">
		<div class="card-content-left">
			<Checkbox
				checked={task.status === "done"}
				disabled={checkboxDisabled}
				aria-label={task.title}
				onchange={(checked) => onToggle(task.id, checked)}
			/>
			<div class="card-text">
				<div class="card-title">{task.title}</div>
				<div class="card-desc">{task.description || "説明なし"}</div>
				<div class="card-meta-row">
					{#if task.start_date}
						<MetaItem icon="event" text={`開始: ${fmtTaskDate(task.start_date)}`} />
					{/if}
					{#if task.due_date}
						<MetaItem
							icon="calendar_today"
							text={`期限: ${fmtTaskDate(task.due_date)}`}
						/>
					{/if}
					<MetaItem
						icon="priority_high"
						text={`優先度: ${priorityLabel(task.priority)}`}
					/>
					{#each tags as tag (tag)}
						<TagChip label={`#${tag}`} />
					{/each}
				</div>
			</div>
		</div>
		<div class="card-actions-right">
			<button type="button" class="btn-trash" onclick={() => onDelete(task.id)}>
				<Icon name="delete" />
			</button>
		</div>
	</div>

	<!-- 進捗バー＋％ -->
	<div class="task-card-progress">
		<ProgressBar {percent} class="task-card-progress-bar" />
		<span class="task-progress-text">
			{#if hasSubtasks}
				{percent}% ({doneCount}/{subtasks.length})
			{:else}
				{percent}%
			{/if}
		</span>
	</div>

	<!-- 操作ボタン -->
	<div class="task-card-buttons">
		{#if !hasSubtasks}
			<button type="button" class="btn-mini" onclick={() => onProgress(task)}>
				<Icon name="trending_up" />進捗更新
			</button>
		{/if}
		<button type="button" class="btn-mini" onclick={() => onAddSub(task)}>
			<Icon name="add_task" />サブタスク
		</button>
		<button type="button" class="btn-mini" onclick={() => onEdit(task)}>
			<Icon name="edit" />編集
		</button>
	</div>

	<!-- サブタスク（折りたたみ） -->
	{#if hasSubtasks}
		<button
			type="button"
			class="subtask-toggle"
			onclick={() => (expanded = !expanded)}
		>
			<Icon name="expand_more" class={expanded ? "" : "subtask-toggle-collapsed"} />
			サブタスク {countDescendants(subtasks)}件
		</button>
		{#if expanded}
			<div class="subtask-list">
				{#each subtasks as sub (sub.id)}
					<SubtaskRow
						{sub}
						{onToggle}
						{onEdit}
						{onAddSub}
						{onDelete}
					/>
				{/each}
			</div>
		{/if}
	{/if}
</div>

<style>
	/* 旧 buildTaskCard の inline style（card.style.flexDirection 等）を再現。 */
	.task-card-col {
		flex-direction: column;
		align-items: stretch;
	}
	.task-card-top {
		display: flex;
		justify-content: space-between;
		gap: 8px;
	}
	.task-card-progress {
		display: flex;
		align-items: center;
	}
	/* 旧 bar.style.flex = "1" 相当。 */
	:global(.task-card-progress .task-card-progress-bar) {
		flex: 1;
	}
	.task-card-buttons {
		margin-top: 8px;
	}
	.subtask-toggle {
		margin-top: 6px;
	}
	/* 旧 tIcon.style.transform = rotate(-90deg)（折りたたみ時）。 */
	:global(.subtask-toggle .subtask-toggle-collapsed) {
		transform: rotate(-90deg);
	}
</style>
