<script lang="ts" module>
// 連携(int) 状態などの status → ラベル/トーンマップ。
// tone は既存 .admin-status-badge の修飾クラス名に写像する:
//   status-active(緑) / status-suspended(赤) / status-default(青) / 素(グレー)
export type ChipStatus =
	| "running" // 稼働中
	| "connected" // 接続中
	| "stopped" // 停止中
	| "unset" // 未設定
	| "error" // エラー
	| "pending"; // 保留

type ChipDef = { label: string; tone: string };

const STATUS_MAP: Record<ChipStatus, ChipDef> = {
	running: { label: "稼働中", tone: "status-active" },
	connected: { label: "接続中", tone: "status-active" },
	stopped: { label: "停止中", tone: "status-suspended" },
	error: { label: "エラー", tone: "status-suspended" },
	unset: { label: "未設定", tone: "" },
	pending: { label: "保留", tone: "status-default" },
};
</script>

<script lang="ts">
	import Badge from "./Badge.svelte";
	import Icon from "./Icon.svelte";

	interface Props {
		/** 既知ステータス。未知値は label/tone を明示指定して使う。 */
		status?: ChipStatus;
		/** マップを上書きする表示ラベル（未知ステータス用） */
		label?: string;
		/** マップを上書きするトーンクラス */
		tone?: string;
		/** 左に表示する Material Symbols アイコン名 */
		icon?: string;
		class?: string;
	}

	let { status, label, tone, icon, class: klass = "" }: Props = $props();

	const def = $derived(status ? STATUS_MAP[status] : undefined);
	const shownLabel = $derived(label ?? def?.label ?? String(status ?? ""));
	const shownTone = $derived(tone ?? def?.tone ?? "");
</script>

<Badge pill tone={shownTone} class={klass}>
	{#if icon}<Icon name={icon} />{/if}{shownLabel}
</Badge>
