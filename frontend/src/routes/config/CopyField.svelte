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

<div class="copy-field plain-fields">
	<label for={fieldId}>{label}</label>
	<div class="copy-row">
		<div class="input-wrap copy-input-wrap">
			<input id={fieldId} type="text" readonly value={value} class="copy-input" />
			<button
				type="button"
				class="inline-button"
				onclick={copy}
				aria-label={copied ? "コピー済み" : "コピー"}
				title={copied ? "コピー済み" : "コピー"}
			>
				<span class="material-symbols-outlined" aria-hidden="true">{copied ? "check" : "content_copy"}</span>
			</button>
		</div>
		<a
			class="icon-action"
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
		margin-right: 8px;
	}
	.copy-input {
		font-family: var(--font-family-mono);
	}
</style>
