<script lang="ts">
// サブタスク行（旧 app.js:2307 buildSubtaskRow）。
// <svelte:self> による再帰で無制限ネストを描画する。
// 完了チェックボックスは「子を持ち算出進捗<100%」で無効化（旧仕様）。

import type { TodoWithSubtasks } from "$lib/api/types";
import { Checkbox, Icon } from "$lib/components/ui";
import Self from "./SubtaskRow.svelte";
import { completionDisabled, fmtTaskDate } from "./taskUtils";

interface Props {
	sub: TodoWithSubtasks;
	depth?: number;
	onToggle: (id: number, checked: boolean) => void;
	onEdit: (task: TodoWithSubtasks) => void;
	onAddSub: (parent: TodoWithSubtasks) => void;
	onDelete: (id: number) => void;
}

let { sub, depth = 0, onToggle, onEdit, onAddSub, onDelete }: Props = $props();

const children = $derived(sub.subtasks || []);
const disabled = $derived(completionDisabled(sub));
</script>

<div class="subtask-group">
	<div
		class="subtask-row"
		class:done={sub.status === "done"}
		style="padding-left:{depth * 16}px"
	>
		<Checkbox
			checked={sub.status === "done"}
			{disabled}
			aria-label={sub.title}
			onchange={(checked) => onToggle(sub.id, checked)}
		/>
		<span class="subtask-title">{sub.title}</span>

		{#if sub.due_date}
			<span class="subtask-due">〜{fmtTaskDate(sub.due_date)}</span>
		{/if}

		<button
			type="button"
			class="btn-icon-sm"
			title="編集"
			onclick={() => onEdit(sub)}
		>
			<Icon name="edit" />
		</button>
		<button
			type="button"
			class="btn-icon-sm"
			title="サブタスクを追加"
			onclick={() => onAddSub(sub)}
		>
			<Icon name="add" />
		</button>
		<button type="button" class="btn-trash" onclick={() => onDelete(sub.id)}>
			<Icon name="delete" />
		</button>
	</div>

	{#if children.length > 0}
		<div class="subtask-list">
			{#each children as child (child.id)}
				<Self
					sub={child}
					depth={depth + 1}
					{onToggle}
					{onEdit}
					{onAddSub}
					{onDelete}
				/>
			{/each}
		</div>
	{/if}
</div>
