<script lang="ts">
// 送り先（DM / チャンネル）を選ぶ枠つき入力。定期報告の報告先と Webhook の通知先で共用する。
// 枠の幅は固定。DM のときはプルダウンが枠いっぱいに広がり、チャンネルにすると
// プルダウンが本来の幅へ縮んで、空いた右側に ID 入力欄が伸びてくる。
import { Select } from "$lib/components/ui";

type TargetType = "dm" | "channel";

interface Props {
	/** プルダウン（トリガーボタン）の id。label の for に使う */
	id: string;
	/** チャンネル ID 入力欄の id */
	idInputId: string;
	type: TargetType;
	targetId: string;
	/** `disabled` の選択肢は表示するが選べない */
	options?: { value: TargetType; label: string; disabled?: boolean }[];
}
let {
	id,
	idInputId,
	type = $bindable(),
	targetId = $bindable(),
	options = [
		{ value: "dm", label: "DM" },
		{ value: "channel", label: "チャンネル" },
	],
}: Props = $props();
</script>

<div class="joined-field" class:open={type === "channel"}>
	<Select {id} class="target-select" {options} bind:value={type} />
	<div class="target-extra" inert={type !== "channel"}>
		<span class="divider" aria-hidden="true"></span>
		<input
			id={idInputId}
			type="text"
			autocomplete="off"
			placeholder="チャンネルID（例: 123456789012345678）"
			aria-label="チャンネルID"
			bind:value={targetId}
		/>
	</div>
</div>

<style>
	.joined-field :global(.target-select) {
		flex: 1 0 auto;
		transition: flex-grow 0.3s ease;
	}
	.joined-field.open :global(.target-select) {
		flex-grow: 0;
	}
	.target-extra {
		display: flex;
		flex: 1 1 0;
		width: 0;
		min-width: 0;
		overflow: hidden;
		flex-grow: 0;
		opacity: 0;
		transition:
			flex-grow 0.3s ease,
			opacity 0.2s ease;
	}
	.joined-field.open .target-extra {
		flex-grow: 1;
		opacity: 1;
	}
	/* 入力欄は Discord 設定ページと同じく枠線・フォーカスリングなし。 */
	.target-extra input,
	.target-extra input:focus {
		flex: 1;
		min-width: 0;
		border: 0;
		outline: none;
		box-shadow: none;
		background: transparent;
	}
</style>
