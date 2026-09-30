<script lang="ts">
// 接続端末1件のカード（旧 buildDeviceCard）。

import type { DesktopDevice } from "$lib/api/services/deviceApi";
import { Icon, MetaItem } from "$lib/components/ui";

interface Props {
	device: DesktopDevice;
	onrevoke: (id: number) => void;
}
let { device, onrevoke }: Props = $props();

const createdText = $derived(`認可: ${(device.created_at || "").slice(0, 16)}`);
const usedText = $derived(
	device.last_used_at
		? `最終利用: ${device.last_used_at.slice(0, 16)}`
		: "最終利用: 未使用",
);
</script>

<div class="card-item glass hover-lift">
	<div class="card-content-left">
		<span class="material-symbols-outlined list-card-icon device-icon">devices</span>
		<div class="card-text">
			<div class="card-title">
				{device.device_name || "不明な端末"}
				{#if device.current}
					<span class="badge badge-accent current-badge">現在の端末</span>
				{/if}
			</div>
			<div class="card-meta-row">
				<MetaItem icon="schedule" text={createdText} />
				<MetaItem icon="history" text={usedText} />
			</div>
		</div>
	</div>
	<div class="card-actions-right">
		<button
			type="button"
			class="btn-trash"
			title="この端末を失効する"
			onclick={() => onrevoke(device.id)}
		>
			<Icon name="delete" />
		</button>
	</div>
</div>

<style>
	.device-icon {
		font-size: 1.8rem;
	}
	.current-badge {
		margin-left: 8px;
	}
</style>
