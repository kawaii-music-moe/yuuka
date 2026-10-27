<script lang="ts">
// cron 式の入力欄。[かんたん / cron式] の切替 + cron 式の入力。
// 「かんたん」を選ぶと組み立てモーダル（CronBuilder）を開き、分/時/日/月/曜日 ごとに選んだ内容から
// cron 式を組み立てて書き込む。今の式をビルダーで表せるときは、その内容から始める。
import { Button, Modal } from "$lib/components/ui";
import {
	buildCron,
	type CronState,
	defaultState,
	parseCron,
} from "$lib/cronBuilder";
import CronBuilder from "./CronBuilder.svelte";
import InlineSelect from "./InlineSelect.svelte";

interface Props {
	id: string;
	value: string;
	placeholder?: string;
	/** 欄の見出し */
	label?: string;
	/** 見出しに必須の * を付ける */
	required?: boolean;
	/** ? のヘルプに出す説明 */
	help?: string;
}
let {
	id,
	value = $bindable(),
	placeholder = "",
	label = "配信スケジュール",
	required = false,
	help = "cron式で細かな設定が可能です！",
}: Props = $props();

// 一番左の切替。ふだんは cron 式の直接入力で、「かんたん」を選ぶと組み立てモーダルが開き、
// 閉じると（決定でもキャンセルでも）cron 式へ戻る。
type Mode = "assist" | "raw";
const MODE_OPTIONS: { value: Mode; label: string }[] = [
	{ value: "assist", label: "かんたん" },
	{ value: "raw", label: "cron式" },
];
let mode = $state<Mode>("raw");

// ? のヘルプはホバーで表示し、クリックで開いたままにできる
let helpOpen = $state(false);

// 組み立てモーダル。開くたびに今の式から下書きを作り、決定で書き込む。
let builderOpen = $state(false);
let draft = $state<CronState>(defaultState());
// 今の式がビルダーで表せず、初期状態から始めたとき
let unparsed = $state("");

function openBuilder() {
	const current = value.trim();
	const parsed = parseCron(current);
	draft = parsed ?? defaultState();
	unparsed = parsed || !current ? "" : current;
	builderOpen = true;
}

function closeBuilder() {
	builderOpen = false;
	mode = "raw";
}

function confirmBuilder() {
	value = buildCron(draft);
	closeBuilder();
}

function onModeChange(m: Mode) {
	if (m === "assist") openBuilder();
}
</script>

<div class="cron-field">
	<div class="cron-label">
		<label for="{id}-mode">{label}{#if required}<span class="required-mark"> *</span>{/if}</label>
		<span class="help">
			<button
				type="button"
				class="help-button"
				aria-label="cron式の説明"
				aria-describedby="{id}-help"
				aria-expanded={helpOpen}
				onclick={() => (helpOpen = !helpOpen)}
				onblur={() => (helpOpen = false)}
			>
				<span class="material-symbols-outlined" aria-hidden="true">help</span>
			</button>
			<span id="{id}-help" role="tooltip" class="help-tip" class:open={helpOpen}>
				{help}
			</span>
		</span>
	</div>
	<div class="joined-field">
		<InlineSelect
			id="{id}-mode"
			class="fixed-select"
			options={MODE_OPTIONS}
			bind:value={mode}
			onchange={onModeChange}
		/>
		<span class="divider" aria-hidden="true"></span>
			<input
				{id}
				type="text"
				autocomplete="off"
				aria-label="cron式"
				{placeholder}
				class="cron-input"
				bind:value
			/>
	</div>
</div>

<Modal bind:open={builderOpen} wide onclose={() => (mode = "raw")} title="スケジュールを組み立てる">
	{#if unparsed}
		<p class="unparsed">
			今の式 <code>{unparsed}</code> はここでは組み立てられないため、初期状態から始めます。決定すると置き換わります。
		</p>
	{/if}
	<CronBuilder id="{id}-builder" bind:schedule={draft} />
	{#snippet footer()}
		<Button variant="secondary" onclick={closeBuilder}>キャンセル</Button>
		<Button variant="primary" onclick={confirmBuilder}>決定</Button>
	{/snippet}
</Modal>

<style>
	.cron-field {
		display: flex;
		flex-direction: column;
		gap: 8px;
		min-width: 0;
	}
	.cron-label {
		display: flex;
		align-items: center;
		gap: 4px;
	}
	/* .form-group label の下余白を外し、? と縦中央を揃える（ラベルと欄の間は .cron-field の gap で取る） */
	.cron-label label {
		margin: 0;
		white-space: nowrap;
		line-height: 1;
	}
	.help {
		position: relative;
		display: flex;
	}
	.help-tip {
		position: absolute;
		left: 50%;
		bottom: calc(100% + 6px);
		transform: translateX(-50%);
		z-index: 20;
		padding: 6px 10px;
		white-space: nowrap;
		font-size: 0.8rem;
		color: var(--text-primary);
		background: var(--surface-1dp);
		border: 1px solid var(--border-divider);
		border-radius: 6px;
		box-shadow: 0 6px 18px rgba(0, 0, 0, 0.18);
		opacity: 0;
		visibility: hidden;
		pointer-events: none;
		transition: opacity 0.15s ease;
	}
	.help:hover .help-tip,
	.help-tip.open {
		opacity: 1;
		visibility: visible;
	}
	.help-button {
		display: flex;
		align-items: center;
		padding: 0;
		border: 0;
		background: transparent;
		color: var(--text-secondary);
		cursor: pointer;
	}
	.help-button .material-symbols-outlined {
		font-size: 18px;
		line-height: 1;
	}
	/* かんたん/cron式 は幅を固定し、文字を中央に置く */
	.joined-field :global(.fixed-select) {
		flex: 0 0 120px;
	}
	.joined-field :global(.fixed-select .trigger) {
		justify-content: center;
	}
	/* 他の入力欄と同じく、フォーカス時も枠線・リングを出さない（input[type=text]:focus より強く当てる） */
	.cron-input,
	.cron-input:focus {
		flex: 1;
		min-width: 0;
		border: 0;
		outline: none;
		box-shadow: none;
		background: transparent;
	}
	.unparsed {
		margin: 0 0 16px;
		font-size: 0.82rem;
		color: var(--text-secondary);
	}
	.unparsed code {
		font-family: var(--font-family-mono);
		color: var(--text-primary);
	}
</style>
