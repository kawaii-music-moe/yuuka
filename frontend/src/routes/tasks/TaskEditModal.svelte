<script lang="ts">
	// タスク新規/編集モーダル（旧 app.js:2619 prepareNewTaskModal + :2629 openEditTaskModal
	//  + :2649 taskForm submit）。editingTask が null なら新規、あれば編集。
	import { Modal, Button } from "$lib/components/ui";
	import type { TodoWithSubtasks, TodoPriority } from "$lib/api/types";

	interface Props {
		open?: boolean;
		/** 編集対象。null なら新規追加。 */
		editingTask?: TodoWithSubtasks | null;
		/** 保存ハンドラ。呼び出し側が API を叩き、成功時に open=false にする。 */
		onsave: (payload: {
			id: number | null;
			title: string;
			description: string;
			startDate: string;
			dueDate: string;
			priority: TodoPriority | null;
		}) => void;
	}

	let { open = $bindable(false), editingTask = null, onsave }: Props = $props();

	let title = $state("");
	let description = $state("");
	let startDate = $state("");
	let dueDate = $state("");
	let priority = $state<TodoPriority | "">("");

	const isEdit = $derived(editingTask != null);

	// モーダルを開くたびにフォームを editingTask で初期化（旧 openEditTaskModal /
	// prepareNewTaskModal の value 代入・reset を再現）。
	$effect(() => {
		if (!open) return;
		const t = editingTask;
		title = t?.title ?? "";
		description = t?.description ?? "";
		startDate = (t?.start_date ?? "").slice(0, 10);
		dueDate = (t?.due_date ?? "").slice(0, 10);
		priority = (t?.priority as TodoPriority | null) ?? "";
	});

	function submit(e: SubmitEvent) {
		e.preventDefault();
		const trimmed = title.trim();
		if (!trimmed) return;
		onsave({
			id: editingTask?.id ?? null,
			title: trimmed,
			description: description.trim(),
			startDate,
			dueDate,
			priority: priority || null,
		});
	}
</script>

<Modal bind:open title={isEdit ? "タスクを編集" : "新規タスクの追加"}>
	<form onsubmit={submit}>
		<div class="form-group">
			<label for="task-title">タイトル *</label>
			<input
				type="text"
				id="task-title"
				required
				placeholder="タスク of Seminar"
				bind:value={title}
			/>
		</div>
		<div class="form-group">
			<label for="task-description">詳細説明</label>
			<textarea
				id="task-description"
				placeholder="タスクのメモ書き"
				bind:value={description}
			></textarea>
		</div>
		<div class="form-row">
			<div class="form-group">
				<label for="task-start">開始日</label>
				<input type="date" id="task-start" bind:value={startDate} />
			</div>
			<div class="form-group">
				<label for="task-due">期限日</label>
				<input type="date" id="task-due" bind:value={dueDate} />
			</div>
		</div>
		<div class="form-group">
			<label for="task-priority">優先度</label>
			<select id="task-priority" bind:value={priority}>
				<option value="">指定なし</option>
				<option value="low">低</option>
				<option value="medium">中</option>
				<option value="high">高</option>
			</select>
		</div>
		<Button type="submit" variant="primary" block>
			{isEdit ? "更新を保存" : "タスク登録"}
		</Button>
	</form>
</Modal>
