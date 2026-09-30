<script lang="ts">
// 新規予定モーダル（旧 index.html #modal-schedule + app.js scheduleForm submit）。
// datetime-local 入力を保持し、保存は onsave で親へ委譲（子は API を叩かない）。
import { Button, Modal } from "$lib/components/ui";
import { toDbDatetime } from "./scheduleUtils";

interface Props {
	open?: boolean;
	/** 保存ハンドラ。親が API を叩き、成功時に open=false にする。 */
	onsave: (payload: {
		title: string;
		description: string;
		startAt: string;
		endAt?: string;
		remindBeforeMinutes: number;
	}) => void;
}

let { open = $bindable(false), onsave }: Props = $props();

let title = $state("");
let description = $state("");
let start = $state("");
let end = $state("");
let remind = $state(30);

// 開くたびに初期化（旧 scheduleForm.reset()）。
$effect(() => {
	if (!open) return;
	title = "";
	description = "";
	start = "";
	end = "";
	remind = 30;
});

function submit(e: SubmitEvent) {
	e.preventDefault();
	const trimmed = title.trim();
	const startAt = toDbDatetime(start);
	if (!trimmed || !startAt) return;
	onsave({
		title: trimmed,
		description: description.trim(),
		startAt,
		endAt: toDbDatetime(end),
		remindBeforeMinutes: Number(remind),
	});
}
</script>

<Modal bind:open title="新規予定の登録">
	<form onsubmit={submit}>
		<div class="form-group">
			<label for="sched-title">タイトル *</label>
			<input
				type="text"
				id="sched-title"
				required
				placeholder="予定の件名"
				bind:value={title}
			/>
		</div>
		<div class="form-group">
			<label for="sched-description">予定詳細</label>
			<textarea
				id="sched-description"
				placeholder="予定のメモ"
				bind:value={description}
			></textarea>
		</div>
		<div class="form-row">
			<div class="form-group">
				<label for="sched-start">開始日時 *</label>
				<input type="datetime-local" id="sched-start" required bind:value={start} />
			</div>
			<div class="form-group">
				<label for="sched-end">終了日時</label>
				<input type="datetime-local" id="sched-end" bind:value={end} />
			</div>
		</div>
		<div class="form-group">
			<label for="sched-remind">何分前にリマインドするか</label>
			<input
				type="number"
				id="sched-remind"
				placeholder="30"
				bind:value={remind}
			/>
		</div>
		<Button type="submit" variant="primary" block>予定登録</Button>
	</form>
</Modal>
