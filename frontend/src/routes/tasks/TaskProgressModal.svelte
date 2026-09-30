<script lang="ts">
// 進捗更新モーダル（旧 app.js:2725 openProgressModal + task-progress-form submit）。
// 手動進捗（0-100, step 5）+ 任意メモ。サブタスクを持つ親では呼ばれない
//  （TaskCard 側で「進捗更新」ボタンを出さない）。

import type { TodoWithSubtasks } from "$lib/api/types";
import { Button, Modal } from "$lib/components/ui";

interface Props {
	open?: boolean;
	task?: TodoWithSubtasks | null;
	onsave: (payload: { id: number; progress: number; note: string }) => void;
}

let { open = $bindable(false), task = null, onsave }: Props = $props();

let progress = $state(0);
let note = $state("");

// 開くたびに task.progress で初期化（旧 range.value = task.progress）。
$effect(() => {
	if (!open) return;
	progress = task?.progress ?? 0;
	note = "";
});

function submit(e: SubmitEvent) {
	e.preventDefault();
	if (task == null) return;
	onsave({ id: task.id, progress: Number(progress), note: note.trim() });
}
</script>

<Modal bind:open title="進捗を更新">
	<form onsubmit={submit}>
		<p class="task-progress-label">タスク: {task?.title ?? ""}</p>
		<div class="form-group">
			<label for="task-progress-range">進捗: {progress}%</label>
			<input
				type="range"
				id="task-progress-range"
				min="0"
				max="100"
				step="5"
				bind:value={progress}
				style="width: 100%;"
			/>
		</div>
		<div class="form-group">
			<label for="task-progress-note">進捗メモ（任意）</label>
			<textarea
				id="task-progress-note"
				placeholder="例: 設計フェーズ完了、実装に着手"
				bind:value={note}
			></textarea>
		</div>
		<Button type="submit" variant="primary" block>進捗を保存</Button>
	</form>
</Modal>

<style>
	.task-progress-label {
		font-size: 0.85rem;
		opacity: 0.8;
		margin-bottom: 0.75rem;
	}
</style>
