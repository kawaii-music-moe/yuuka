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

<div class="form-group copy-field">
	<label for={fieldId}>{label}</label>
	<div class="copy-row">
		<input id={fieldId} type="text" readonly value={value} class="copy-input" />
		<button type="button" class="btn btn-secondary" onclick={copy}>
			{copied ? "コピー済み" : "コピー"}
		</button>
		<a class="btn btn-primary" href={value} target="_blank" rel="noopener">開く</a>
	</div>
	{#if sub}
		<span class="field-sub">{sub}</span>
	{/if}
</div>

<style>
	.copy-field {
		margin-bottom: 0;
	}
	.copy-row {
		display: flex;
		gap: 12px;
	}
	.copy-input {
		flex-grow: 1;
		font-family: var(--font-family-mono);
	}
	.copy-row .btn {
		white-space: nowrap;
	}
</style>
