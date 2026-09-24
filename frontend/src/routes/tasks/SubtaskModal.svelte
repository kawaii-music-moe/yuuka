<script lang="ts">
// サブタスク追加モーダル（旧 app.js:2684 openSubtaskModal + subtask-form submit）。
// parent の下に parentId 付きで /api/tasks/add する。

import type { TodoWithSubtasks } from "$lib/api/types";
import { Button, Modal } from "$lib/components/ui";

interface Props {
	open?: boolean;
	/** 親タスク（サブタスク登録先）。 */
	parent?: TodoWithSubtasks | null;
	onsave: (payload: {
		parentId: number;
		title: string;
		startDate: string;
		dueDate: string;
	}) => void;
}

let { open = $bindable(false), parent = null, onsave }: Props = $props();

let title = $state("");
let startDate = $state("");
let dueDate = $state("");

// 開くたびに reset（旧 form.reset()）。
$effect(() => {
	if (!open) return;
	title = "";
	startDate = "";
	dueDate = "";
});

function submit(e: SubmitEvent) {
	e.preventDefault();
	const trimmed = title.trim();
	if (!trimmed || parent == null) return;
	onsave({ parentId: parent.id, title: trimmed, startDate, dueDate });
}
</script>

<Modal bind:open title="サブタスクの追加">
	<form onsubmit={submit}>
		<p class="subtask-parent-label">親タスク: {parent?.title ?? ""}</p>
		<div class="form-group">
			<label for="subtask-title">タイトル *</label>
			<input
				type="text"
				id="subtask-title"
				required
				placeholder="サブタスク名"
				bind:value={title}
			/>
		</div>
		<div class="form-row">
			<div class="form-group">
				<label for="subtask-start">開始日</label>
				<input type="date" id="subtask-start" bind:value={startDate} />
			</div>
			<div class="form-group">
				<label for="subtask-due">期限日</label>
				<input type="date" id="subtask-due" bind:value={dueDate} />
			</div>
		</div>
		<Button type="submit" variant="primary" block>サブタスク登録</Button>
	</form>
</Modal>

<style>
	.subtask-parent-label {
		font-size: 0.85rem;
		opacity: 0.8;
		margin-bottom: 0.75rem;
	}
</style>
