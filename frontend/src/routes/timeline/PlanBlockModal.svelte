<script lang="ts">
// 計画ブロック 追加/編集モーダル（旧 app.js openPlanBlockModal + plan-block-form）。
// 子モーダルは自前 state を持ち、open のたび editingBlock で初期化。
// 保存は onsave コールバックで親へ委譲（子は API を叩かない）。
import type { DayPlanBlock, PlanBlockType } from "$lib/api/types";
import { Button, Modal } from "$lib/components/ui";
import { PLAN_TYPE_LABEL } from "./timelineUtils";

export interface PlanBlockFormPayload {
	id: number | null;
	type: PlanBlockType;
	title: string;
	startTime: string;
	endTime: string;
	description: string;
	transitFrom: string;
	transitTo: string;
	transitLine: string;
}

interface Props {
	open?: boolean;
	editingBlock?: DayPlanBlock | null;
	onsave: (payload: PlanBlockFormPayload) => void;
}

let { open = $bindable(false), editingBlock = null, onsave }: Props = $props();

const TYPES: PlanBlockType[] = ["event", "task", "transit", "free"];

let type = $state<PlanBlockType>("event");
let title = $state("");
let startTime = $state("");
let endTime = $state("");
let description = $state("");
let transitFrom = $state("");
let transitTo = $state("");
let transitLine = $state("");

const isEdit = $derived(editingBlock != null);

// 開くたびに editingBlock で初期化（旧 openPlanBlockModal のフォーム埋め）。
$effect(() => {
	if (!open) return;
	const b = editingBlock;
	type = b?.type ?? "event";
	title = b?.title ?? "";
	startTime = b?.start_time ?? "";
	endTime = b?.end_time ?? "";
	description = b?.description ?? "";
	transitFrom = b?.transit_from ?? "";
	transitTo = b?.transit_to ?? "";
	transitLine = b?.transit_line ?? "";
});

function onSubmit(e: SubmitEvent) {
	e.preventDefault();
	if (!title.trim()) return;
	onsave({
		id: editingBlock?.id ?? null,
		type,
		title: title.trim(),
		startTime,
		endTime,
		description,
		transitFrom,
		transitTo,
		transitLine,
	});
}
</script>

<Modal bind:open title={isEdit ? "計画ブロックを編集" : "計画ブロックを追加"}>
	<form class="plan-block-form" onsubmit={onSubmit}>
		<div class="form-group">
			<span class="form-label-text">タイプ</span>
			<div class="tl-type-pills">
				{#each TYPES as t (t)}
					<button
						type="button"
						class="tl-type-pill"
						class:active={type === t}
						onclick={() => (type = t)}>{PLAN_TYPE_LABEL[t]}</button
					>
				{/each}
			</div>
		</div>
		<div class="form-group">
			<label for="plan-block-title">タイトル *</label>
			<input
				type="text"
				id="plan-block-title"
				required
				placeholder="例: 打ち合わせ"
				bind:value={title}
			/>
		</div>
		<div class="form-row">
			<div class="form-group">
				<label for="plan-block-start">開始時刻</label>
				<input type="time" id="plan-block-start" bind:value={startTime} />
			</div>
			<div class="form-group">
				<label for="plan-block-end">終了時刻</label>
				<input type="time" id="plan-block-end" bind:value={endTime} />
			</div>
		</div>
		{#if type === "transit"}
			<div class="form-group">
				<span class="form-label-text">移動情報</span>
				<div class="form-row">
					<input type="text" placeholder="出発地" bind:value={transitFrom} />
					<input type="text" placeholder="到着地" bind:value={transitTo} />
				</div>
				<input
					type="text"
					placeholder="路線・交通機関名"
					class="plan-transit-line"
					bind:value={transitLine}
				/>
			</div>
		{/if}
		<div class="form-group">
			<label for="plan-block-desc">メモ</label>
			<textarea
				id="plan-block-desc"
				rows="2"
				placeholder="補足など"
				bind:value={description}
			></textarea>
		</div>
		<Button variant="primary" type="submit" block>保存</Button>
	</form>
</Modal>

<style>
	.plan-block-form {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}
	.form-label-text {
		display: block;
		margin-bottom: 6px;
		font-size: 0.85rem;
	}
	.plan-transit-line {
		margin-top: 6px;
	}
</style>
