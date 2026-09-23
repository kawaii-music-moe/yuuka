<script lang="ts">
// 記録カード（旧 app.js buildRecordCard）。

import { timelineApi } from "$lib/api/services";
import type { TimelineRecord } from "$lib/api/types";
import { Icon } from "$lib/components/ui";
import { recordTime, recordTypeIcon, recordTypeLabel } from "./timelineUtils";

interface Props {
	record: TimelineRecord;
	ondelete: (id: number) => void;
}

let { record, ondelete }: Props = $props();

const timeStr = $derived(recordTime(record.recorded_at));
const label = $derived(recordTypeLabel(record.type));
const metaText = $derived(timeStr ? `${timeStr} · ${label}` : label);
const isVideo = $derived(record.media_type === "video");
</script>

<div class="tl-record-card" data-type={record.type}>
	<div class="tl-record-top">
		<Icon name={recordTypeIcon(record.type)} class="tl-record-icon" />
		<span class="tl-record-time">{metaText}</span>
	</div>

	{#if record.title}
		<div class="tl-record-title">{record.title}</div>
	{/if}
	{#if record.content}
		<div class="tl-record-content">{record.content}</div>
	{/if}
	{#if record.type === "expense" && record.amount != null}
		<div class="tl-record-title">
			¥{Number(record.amount).toLocaleString()} · {record.expense_category ?? ""}
		</div>
	{/if}
	{#if record.media_path}
		{#if isVideo}
			<!-- svelte-ignore a11y_media_has_caption -->
			<video
				class="tl-record-media"
				src={timelineApi.mediaUrl(record.media_path)}
				controls
				muted
			></video>
		{:else}
			<img
				class="tl-record-media"
				src={timelineApi.mediaUrl(record.media_path)}
				alt={record.title ?? "記録メディア"}
			/>
		{/if}
	{/if}
	{#if record.location}
		<div class="tl-plan-sub">📍 {record.location}</div>
	{/if}

	<div class="tl-record-actions">
		<button
			type="button"
			class="btn-icon-sm tl-del-btn"
			aria-label="削除"
			onclick={() => ondelete(record.id)}
		>
			<Icon name="delete" />
		</button>
	</div>
</div>

<style>
	.tl-record-top {
		display: flex;
		align-items: center;
		gap: 6px;
	}
	.tl-record-top :global(.tl-record-icon) {
		font-size: 0.9rem;
		flex-shrink: 0;
	}
	.tl-del-btn {
		color: var(--color-red);
	}
</style>
