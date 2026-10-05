<script lang="ts">
// 記録 追加モーダル（旧 app.js timeline-record-form + btn-add-record の初期化）。
// タイプ（memo/expense/task_done/media/location）に応じてフィールドを出し分け。
// 保存は onsave コールバックで親へ委譲（media は File を渡し、親が base64 化して送信）。
import type { RecordType } from "$lib/api/types";
import { Button, Modal } from "$lib/components/ui";
import { RECORD_TYPE_LABEL } from "./timelineUtils";

export type RecordFormPayload =
	| {
			kind: "record";
			type: Exclude<RecordType, "media">;
			title: string;
			content: string;
			location: string;
			amount: number;
			category: string;
	  }
	| {
			kind: "media";
			file: File;
			title: string;
			location: string;
	  };

interface Props {
	open?: boolean;
	onsave: (payload: RecordFormPayload) => void;
}

let { open = $bindable(false), onsave }: Props = $props();

const TYPES: RecordType[] = [
	"memo",
	"expense",
	"task_done",
	"media",
	"location",
];
const CATEGORIES = [
	"食費",
	"交通費",
	"日用品",
	"娯楽",
	"医療費",
	"衣服",
	"通信費",
	"光熱費",
	"その他",
];

let type = $state<RecordType>("memo");
let title = $state("");
let content = $state("");
let location = $state("");
let amount = $state<number | null>(null);
let category = $state("食費");
let mediaFile = $state<File | null>(null);
let fileInput = $state<HTMLInputElement | null>(null);

// 開くたびに初期化（旧 btn-add-record ハンドラのリセット）。
$effect(() => {
	if (!open) return;
	type = "memo";
	title = "";
	content = "";
	location = "";
	amount = null;
	category = "食費";
	mediaFile = null;
	if (fileInput) fileInput.value = "";
});

function onFileChange(e: Event) {
	const input = e.currentTarget as HTMLInputElement;
	mediaFile = input.files?.[0] ?? null;
}

function onSubmit(e: SubmitEvent) {
	e.preventDefault();
	if (type === "media") {
		if (!mediaFile) return;
		onsave({
			kind: "media",
			file: mediaFile,
			title: title.trim(),
			location: location.trim(),
		});
		return;
	}
	if (type === "expense" && (amount == null || amount <= 0)) return;
	onsave({
		kind: "record",
		type,
		title: title.trim(),
		content: content.trim(),
		location: location.trim(),
		amount: amount ?? 0,
		category,
	});
}
</script>

<Modal bind:open title="記録を追加">
	<form class="record-form" onsubmit={onSubmit}>
		<div class="form-group">
			<span class="form-label-text">タイプ</span>
			<div class="tl-type-pills">
				{#each TYPES as t (t)}
					<button
						type="button"
						class="tl-type-pill"
						class:active={type === t}
						onclick={() => (type = t)}>{RECORD_TYPE_LABEL[t]}</button
					>
				{/each}
			</div>
		</div>
		<div class="form-group">
			<label for="record-title">見出し</label>
			<input
				type="text"
				id="record-title"
				placeholder="見出し（任意）"
				bind:value={title}
			/>
		</div>

		{#if type !== "expense" && type !== "media"}
			<div class="form-group">
				<label for="record-content">内容</label>
				<textarea
					id="record-content"
					rows="3"
					placeholder="メモ内容"
					bind:value={content}
				></textarea>
			</div>
		{/if}

		{#if type === "expense"}
			<div class="form-group">
				<span class="form-label-text">支出</span>
				<div class="form-row">
					<div class="form-group">
						<label for="record-amount">金額 (円) *</label>
						<input
							type="number"
							id="record-amount"
							min="1"
							placeholder="1000"
							bind:value={amount}
						/>
					</div>
					<div class="form-group">
						<label for="record-category">カテゴリ</label>
						<select id="record-category" bind:value={category}>
							{#each CATEGORIES as c (c)}
								<option value={c}>{c}</option>
							{/each}
						</select>
					</div>
				</div>
			</div>
		{/if}

		{#if type === "media"}
			<div class="form-group">
				<label for="record-media-file">ファイル選択</label>
				<input
					type="file"
					id="record-media-file"
					accept="image/*,video/*"
					bind:this={fileInput}
					onchange={onFileChange}
				/>
			</div>
		{/if}

		<div class="form-group">
			<label for="record-location">場所（任意）</label>
			<input
				type="text"
				id="record-location"
				placeholder="渋谷、カフェ名など"
				bind:value={location}
			/>
		</div>
		<Button variant="primary" type="submit" block>記録する</Button>
	</form>
</Modal>

<style>
	.record-form {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}
	.form-label-text {
		display: block;
		margin-bottom: 6px;
		font-size: 0.85rem;
	}
</style>
