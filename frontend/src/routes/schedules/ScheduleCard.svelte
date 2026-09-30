<script lang="ts">
// 予定1件のカード（旧 app.js:3394 fetchSchedulesList の card 生成部）。
// event アイコン＋タイトル＋説明＋日時メタ＋Google同期バッジ＋削除ボタン。

import type { ScheduleRecord } from "$lib/api/types";
import { Icon, MetaItem } from "$lib/components/ui";
import { formatScheduleRange } from "./scheduleUtils";

interface Props {
	schedule: ScheduleRecord;
	onDelete: (id: number) => void;
}

let { schedule, onDelete }: Props = $props();
</script>

<div class="card-item glass hover-lift">
	<div class="card-content-left">
		<span class="material-symbols-outlined list-card-icon" aria-hidden="true"
			>event</span
		>
		<div class="card-text">
			<div class="card-title">{schedule.title}</div>
			<div class="card-desc">{schedule.description || "説明なし"}</div>
			<div class="card-meta-row">
				<MetaItem
					icon="schedule"
					text={formatScheduleRange(schedule.start_at, schedule.end_at)}
				/>
				{#if schedule.google_calendar_id}
					<MetaItem icon="sync" text="Google同期済み" />
				{/if}
			</div>
		</div>
	</div>
	<div class="card-actions-right">
		<button type="button" class="btn-trash" onclick={() => onDelete(schedule.id)}>
			<Icon name="delete" />
		</button>
	</div>
</div>

<style>
	/* 旧 icon.style.fontSize = "1.8rem" を再現。 */
	.list-card-icon {
		font-size: 1.8rem;
	}
</style>
