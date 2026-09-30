<script lang="ts">
// 共通チェックボックス。既存 .checkbox-custom（テーマ連動・描画チェックマーク）を再利用。
interface Props {
	/** チェック状態（双方向バインド） */
	checked?: boolean;
	disabled?: boolean;
	/** 隣接ラベル文字列（省略時はチェックボックス単体） */
	label?: string;
	id?: string;
	"aria-label"?: string;
	class?: string;
	onchange?: (checked: boolean) => void;
}

let {
	checked = $bindable(false),
	disabled = false,
	label,
	id,
	"aria-label": ariaLabel,
	class: klass = "",
	onchange,
}: Props = $props();

function handle(e: Event) {
	const el = e.currentTarget as HTMLInputElement;
	onchange?.(el.checked);
}
</script>

{#if label}
	<label class="checkbox-label {klass}">
		<input
			type="checkbox"
			class="checkbox-custom"
			bind:checked
			{disabled}
			{id}
			aria-label={ariaLabel ?? label}
			onchange={handle}
		/>
		<span>{label}</span>
	</label>
{:else}
	<input
		type="checkbox"
		class="checkbox-custom {klass}"
		bind:checked
		{disabled}
		{id}
		aria-label={ariaLabel}
		onchange={handle}
	/>
{/if}

<style>
	.checkbox-label {
		display: inline-flex;
		align-items: center;
		gap: 8px;
		cursor: pointer;
		font-size: 0.85rem;
	}
</style>
