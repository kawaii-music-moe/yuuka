<script lang="ts">
// 単一リマインダーカード（旧 app.js fetchRemindersList のカード生成を移植）。
import type { ReminderRecord } from "$lib/api/types";
import { reminderStatusLabel, reminderTargetText } from "./reminderUtils";

interface Props {
	reminder: ReminderRecord;
	oncancel: (id: number) => void;
}

let { reminder, oncancel }: Props = $props();
</script>

<div class="card-item glass reminder-card">
	<div class="reminder-left">
		<div class="reminder-msg-row">
			<span class="card-title reminder-msg">{reminder.message}</span>
			<span class="status-badge status-{reminder.status}"
				>{reminderStatusLabel(reminder.status)}</span
			>
		</div>
		<div class="reminder-meta-row">
			<span>⏰ {reminder.trigger_at}</span>
			{#if reminder.repeat_rule}
				<span class="reminder-repeat">🔁 {reminder.repeat_rule}</span>
			{/if}
			<span>{reminderTargetText(reminder)}</span>
		</div>
	</div>

	{#if reminder.status === "pending"}
		<button
			type="button"
			class="btn btn-secondary btn-sm reminder-cancel-btn"
			onclick={() => oncancel(reminder.id)}>キャンセル</button
		>
	{/if}
</div>

<style>
	.reminder-card {
		align-items: flex-start;
		gap: 10px;
		padding: 12px 14px;
	}
	.reminder-left {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 4px;
	}
	.reminder-msg-row {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
	}
	.reminder-msg {
		font-size: 0.92rem;
	}
	.reminder-meta-row {
		font-size: 0.78rem;
		color: var(--color-zinc-muted);
		display: flex;
		gap: 12px;
		flex-wrap: wrap;
	}
	.reminder-repeat {
		font-family: var(--font-family-mono);
	}
	.reminder-cancel-btn {
		font-size: 0.72rem;
		padding: 3px 10px;
		flex-shrink: 0;
	}
</style>
