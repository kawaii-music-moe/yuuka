<script lang="ts">
// cron 式の入力欄。[かんたん / cron式] の切替 + cron 式の入力。
// 「かんたん」を選ぶと支援モーダルを開き、[毎日/毎週/毎月] + [時:分] + [曜日（毎週のみ）] から
// cron 式を組み立てて書き込む。
import { Button, Modal } from "$lib/components/ui";
import InlineSelect from "./InlineSelect.svelte";
import WeekdaySelect from "./WeekdaySelect.svelte";

interface Props {
	id: string;
	value: string;
	placeholder?: string;
}
let { id, value = $bindable(), placeholder = "" }: Props = $props();

type Freq = "daily" | "weekly" | "monthly";
type Parts = { freq: Freq; hour: string; minute: string; days: number[] };

const FREQ_OPTIONS: { value: Freq; label: string }[] = [
	{ value: "daily", label: "毎日" },
	{ value: "weekly", label: "毎週" },
	{ value: "monthly", label: "毎月" },
];
const HOUR_OPTIONS = Array.from({ length: 24 }, (_, h) => ({
	value: String(h),
	label: `${String(h).padStart(2, "0")}時`,
}));
const MINUTE_OPTIONS = Array.from({ length: 12 }, (_, i) => ({
	value: String(i * 5),
	label: `${String(i * 5).padStart(2, "0")}分`,
}));
const DAY_NAMES = ["日", "月", "火", "水", "木", "金", "土"];

/** 支援で表せる形（分 時 [*|1] * [*|曜日リスト]）なら分解する。表せなければ null。 */
function parse(cron: string): Parts | null {
	const m = cron
		.trim()
		.match(/^(\d{1,2}) (\d{1,2}) (\*|1) \* (\*|[0-6](?:,[0-6])*)$/);
	if (!m) return null;
	const [, mi, h, dom, dow] = m;
	if (Number(h) > 23 || Number(mi) > 59) return null;
	const base = { hour: String(Number(h)), minute: String(Number(mi)) };
	if (dom === "1" && dow === "*")
		return { ...base, freq: "monthly", days: [1] };
	if (dom === "*" && dow !== "*") {
		const days = [...new Set(dow.split(",").map(Number))].sort((a, b) => a - b);
		return { ...base, freq: "weekly", days };
	}
	if (dom === "*") return { ...base, freq: "daily", days: [1] };
	return null;
}

function build(p: Parts): string {
	const dow = p.freq === "weekly" && p.days.length > 0 ? p.days.join(",") : "*";
	const dom = p.freq === "monthly" ? "1" : "*";
	return `${p.minute} ${p.hour} ${dom} * ${dow}`;
}

function describe(p: Parts): string {
	const time = `${p.hour.padStart(2, "0")}:${p.minute.padStart(2, "0")}`;
	if (p.freq === "monthly") return `毎月1日 ${time}`;
	if (p.freq === "weekly")
		return `毎週 ${[1, 2, 3, 4, 5, 6, 0]
			.filter((d) => p.days.includes(d))
			.map((d) => DAY_NAMES[d])
			.join("・")} ${time}`;
	return `毎日 ${time}`;
}

const parsed = $derived(parse(value));

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
let draft = $state<Parts>({ freq: "daily", hour: "7", minute: "0", days: [1] });

function openBuilder() {
	draft = parsed
		? { ...parsed, days: [...parsed.days] }
		: { freq: "daily", hour: "7", minute: "0", days: [1] };
	builderOpen = true;
}

function confirmBuilder() {
	value = build(draft);
	builderOpen = false;
	mode = "raw";
}

function onModeChange(m: Mode) {
	if (m === "assist") openBuilder();
}
</script>

<div class="cron-field">
	<div class="cron-label">
		<label for="{id}-mode">配信スケジュール</label>
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
				cron式で細かな設定が可能です！
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

<Modal bind:open={builderOpen} onclose={() => (mode = "raw")} title="配信スケジュールを組み立てる">
	<div class="builder">
		<div class="builder-row">
			<span class="builder-label">頻度</span>
			<div class="joined-field">
				<InlineSelect
					id="{id}-freq"
					class="grow-select"
					ariaLabel="頻度"
					options={FREQ_OPTIONS}
					bind:value={draft.freq}
				/>
			</div>
		</div>
		<div class="builder-row">
			<span class="builder-label">時刻</span>
			<div class="joined-field">
				<InlineSelect
					id="{id}-hour"
					class="grow-select"
					ariaLabel="時"
					options={HOUR_OPTIONS}
					bind:value={draft.hour}
				/>
				<span class="colon" aria-hidden="true">:</span>
				<InlineSelect
					id="{id}-minute"
					class="grow-select"
					ariaLabel="分"
					options={MINUTE_OPTIONS}
					bind:value={draft.minute}
				/>
			</div>
		</div>
		{#if draft.freq === "weekly"}
			<div class="builder-row">
				<span class="builder-label">曜日</span>
				<div class="joined-field">
					<WeekdaySelect id="{id}-days" bind:value={draft.days} />
				</div>
			</div>
		{/if}
		<p class="builder-preview">
			{describe(draft)}<span class="mono">（{build(draft)}）</span>
		</p>
	</div>
	{#snippet footer()}
		<Button
			variant="secondary"
			onclick={() => {
				builderOpen = false;
				mode = "raw";
			}}
		>キャンセル</Button>
		<Button
			variant="primary"
			disabled={draft.freq === "weekly" && draft.days.length === 0}
			onclick={confirmBuilder}
		>
			決定
		</Button>
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
	.joined-field {
		display: flex;
		min-width: 0;
		background-color: var(--surface-1dp);
		border-radius: 4px;
	}
	.divider {
		width: 1px;
		flex-shrink: 0;
		margin: 8px 0 8px 12px;
		background-color: var(--border-divider);
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
	/* 組み立てモーダル */
	.builder {
		display: flex;
		flex-direction: column;
		gap: 16px;
	}
	.builder-row {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}
	.builder-label {
		font-size: 0.72rem;
		font-weight: 700;
		letter-spacing: 0.05em;
		color: var(--text-secondary);
	}
	.builder .joined-field > :global(.grow-select),
	.builder .joined-field > :global(.inline-select) {
		flex: 1;
	}
	.colon {
		align-self: center;
		color: var(--text-secondary);
	}
	.builder-preview {
		margin: 0;
		font-size: 0.9rem;
		color: var(--text-primary);
	}
	.mono {
		margin-left: 4px;
		font-family: var(--font-family-mono);
		color: var(--text-secondary);
	}
</style>
