<script lang="ts">
// 読み取り専用の URL 入力 + コピー + 開くボタン（旧 copyInputValue + 招待/プロフィール行）。
interface Props {
	label: string;
	value: string;
	sub?: string;
}
let { label, value, sub }: Props = $props();

// label と input を関連付けるための一意 id。
const fieldId = `copy-${Math.random().toString(36).slice(2, 9)}`;

let copied = $state(false);
let timer: ReturnType<typeof setTimeout> | undefined;

async function copy() {
	try {
		await navigator.clipboard.writeText(value);
	} catch {
		/* clipboard 不可でも UI は継続（旧 execCommand フォールバックは省略） */
	}
	copied = true;
	clearTimeout(timer);
	timer = setTimeout(() => (copied = false), 1500);
}
</script>

<div class="copy-field">
	<label for={fieldId}>{label}</label>
	<div class="copy-row">
		<div class="copy-input-wrap">
			<input id={fieldId} type="text" readonly value={value} class="copy-input" />
			<button
				type="button"
				class="icon-button copy-button"
				onclick={copy}
				aria-label={copied ? "コピー済み" : "コピー"}
				title={copied ? "コピー済み" : "コピー"}
			>
				<span class="material-symbols-outlined" aria-hidden="true">{copied ? "check" : "content_copy"}</span>
			</button>
		</div>
		<a
			class="icon-button"
			href={value}
			target="_blank"
			rel="noopener"
			aria-label="開く"
			title="開く"
		>
			<span class="material-symbols-outlined" aria-hidden="true">open_in_new</span>
		</a>
	</div>
	{#if sub}
		<span class="field-sub">{sub}</span>
	{/if}
</div>

<style>
	.copy-field label {
		font-size: 0.9rem;
		color: var(--text-primary);
	}
	.copy-field {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}
	.copy-row {
		display: flex;
		gap: 4px;
	}
	.copy-input-wrap {
		position: relative;
		flex: 1;
		min-width: 0;
		margin-right: 8px;
	}
	.copy-input {
		width: 100%;
		box-sizing: border-box;
		padding-right: 44px;
		font-family: var(--font-family-mono);
		border: 0;
		outline: none;
		box-shadow: none;
	}
	.copy-input:focus {
		border: 0;
		outline: none;
		box-shadow: none;
	}
	.icon-button {
		display: flex;
		align-items: center;
		justify-content: center;
		width: 40px;
		flex-shrink: 0;
		border: 0;
		border-radius: 6px;
		background: transparent;
		color: var(--color-primary);
		cursor: pointer;
		text-decoration: none;
	}
	.copy-button {
		position: absolute;
		right: 4px;
		top: 50%;
		transform: translateY(-50%);
		width: 36px;
		height: 32px;
		color: var(--text-secondary);
	}
</style>
