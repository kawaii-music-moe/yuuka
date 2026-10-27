<script lang="ts">
// cron 式ビルダー。左で 分/時/日/月/曜日 のタブごとに指定方法（毎回・カスタム・間隔・範囲）を選んで
// 値を足していき、右に組み上がった式・フィールドごとの内訳・説明文を出す。
// 分・時は数字を入力し、日・月・曜日はボタン（カスタム）やプルダウン（間隔・範囲）で選ぶ。
// 式の組み立て・分解は $lib/cronBuilder（バックエンドの croner と同じ 5 フィールド）。

import { slide } from "svelte/transition";
import { PageTabs } from "$lib/components/ui";
import {
	buildCron,
	type CronState,
	defaultState,
	describeCron,
	FIELDS,
	type FieldIndex,
	type FieldMode,
	fieldExpr,
	hasDomAndDow,
	intervalLabel,
	parseBounded,
	rangeLabel,
	valueLabel,
	WEEKDAYS,
} from "$lib/cronBuilder";
import InlineSelect from "./InlineSelect.svelte";

interface Props {
	id: string;
	/** ビルダーの状態（呼び出し側が bind して、決定時に buildCron で式にする） */
	schedule: CronState;
}
let { id, schedule = $bindable() }: Props = $props();

const TABS = FIELDS.map((f, i) => ({ id: String(i), label: f.name }));
let activeTab = $state("0");
const active = $derived(Number(activeTab) as FieldIndex);
const field = $derived(schedule[active]);
const spec = $derived(FIELDS[active]);
// 分・時は数字の入力欄、日・月・曜日は選択式
const typed = $derived(active === 0 || active === 1);
// 間隔の単位（入力欄の後ろに出す）
const stepUnit = $derived(active === 0 ? "分" : "時間");

const INTERVAL_HINTS = [
	"各時の 0 分から数えます。",
	"各日の 0 時から数えます。",
	"毎月 1 日から数えます。月をまたいで一定の日数おきになるわけではありません。",
	"1 月から数えます。",
	"日曜日（0）から曜日の番号で数えます。一定の日数おきではありません。",
];

const modes = $derived<{ value: FieldMode; label: string }[]>([
	{ value: "every", label: spec.everyLabel },
	{ value: "custom", label: "カスタム" },
	{ value: "interval", label: "間隔" },
	{ value: "range", label: "範囲" },
]);

const opts = (from: number, to: number, label: (v: number) => string) =>
	Array.from({ length: to - from + 1 }, (_, i) => ({
		value: String(from + i),
		label: label(from + i),
	}));

// 追加前の入力・選択値（タブを切り替えたら、そのフィールドの既定に戻す）。
// 入力欄（分・時）は空から始め、placeholder で入れられる範囲を見せる。
let pickValue = $state("");
let pickStep = $state("");
let pickStart = $state("");
let pickEnd = $state("");
let pickRangeStep = $state("");
let inputError = $state("");

$effect(() => {
	const { min, max } = FIELDS[active];
	const isTyped = active === 0 || active === 1;
	pickValue = "";
	pickStep = isTyped ? "" : "2";
	pickStart = isTyped ? "" : String(min);
	pickEnd = isTyped ? "" : String(max);
	pickRangeStep = isTyped ? "" : "1";
	inputError = "";
});

const valueOptions = $derived(
	opts(spec.min, spec.max, (v) => valueLabel(active, v)),
);
const stepOptions = $derived(
	opts(2, spec.max, (v) => intervalLabel(active, v).replace(/ごと$/, "")),
);
const rangeStepOptions = $derived(
	opts(1, spec.max - spec.min + 1, (v) =>
		v === 1 ? "すべて" : `${intervalLabel(active, v).replace(/ごと$/, "")}おき`,
	),
);

function outOfRange(min: number, max: number): string {
	return `${min}〜${max} の数字を入力してください。`;
}

function setMode(m: FieldMode) {
	field.mode = m;
	inputError = "";
}

// プルダウンの右に入力欄を出すか（毎回と、日・月・曜日のカスタムは出さない）
const paneOpen = $derived(
	field.mode === "interval" ||
		field.mode === "range" ||
		(field.mode === "custom" && typed),
);

// 追加ボタンは、必要な欄が入力されるまで押せない（範囲の間隔は空なら 1 なので任意）
const canAdd = $derived(
	field.mode === "custom"
		? pickValue.trim() !== ""
		: field.mode === "interval"
			? pickStep.trim() !== ""
			: pickStart.trim() !== "" && pickEnd.trim() !== "",
);

function toggleValue(v: number) {
	field.values = field.values.includes(v)
		? field.values.filter((x) => x !== v)
		: [...field.values, v];
}

function addValue() {
	const v = parseBounded(pickValue, spec.min, spec.max);
	if (v === null) {
		inputError = outOfRange(spec.min, spec.max);
		return;
	}
	inputError = "";
	pickValue = "";
	if (!field.values.includes(v)) field.values = [...field.values, v];
}

function addInterval() {
	const v = parseBounded(pickStep, 2, spec.max);
	if (v === null) {
		inputError = outOfRange(2, spec.max);
		return;
	}
	inputError = "";
	if (!field.intervals.includes(v)) field.intervals = [...field.intervals, v];
}

function addRange() {
	const start = parseBounded(pickStart, spec.min, spec.max);
	const end = parseBounded(pickEnd, spec.min, spec.max);
	const maxStep = spec.max - spec.min + 1;
	// 間隔は空なら 1（範囲内すべて）
	const step =
		pickRangeStep.trim() === "" ? 1 : parseBounded(pickRangeStep, 1, maxStep);
	if (start === null || end === null) {
		inputError = outOfRange(spec.min, spec.max);
		return;
	}
	if (step === null) {
		inputError = `間隔は ${outOfRange(1, maxStep)}`;
		return;
	}
	if (start > end) {
		inputError = "終了は開始以上の値にしてください。";
		return;
	}
	inputError = "";
	if (
		!field.ranges.some(
			(x) => x.start === start && x.end === end && x.step === step,
		)
	)
		field.ranges = [...field.ranges, { start, end, step }];
}

/** 入力欄で Enter を押したら追加する */
function onEnter(add: () => void) {
	return (e: KeyboardEvent) => {
		if (e.key !== "Enter" || e.isComposing) return;
		e.preventDefault();
		if (!canAdd) return;
		add();
	};
}

// 日・月・曜日の値はボタンの選択状態で分かるので、タグには間隔・範囲だけを出す
const chips = $derived([
	...field.ranges.map((r, i) => ({
		key: `r${i}`,
		label: rangeLabel(active, r),
		remove: () => {
			field.ranges = field.ranges.filter((_, j) => j !== i);
		},
	})),
	...[...field.intervals]
		.sort((a, b) => a - b)
		.map((v) => ({
			key: `i${v}`,
			label: intervalLabel(active, v),
			remove: () => {
				field.intervals = field.intervals.filter((x) => x !== v);
			},
		})),
	...(typed ? [...field.values] : [])
		.sort((a, b) => a - b)
		.map((v) => ({
			key: `v${v}`,
			label: valueLabel(active, v),
			remove: () => {
				field.values = field.values.filter((x) => x !== v);
			},
		})),
]);
// 日・月・曜日のカスタムはボタンが選択状態を表すので、タグ欄自体を出さない
const showChips = $derived(
	field.mode === "interval" ||
		field.mode === "range" ||
		(field.mode === "custom" && (typed || chips.length > 0)),
);

// 日・月・曜日のカスタムは、ボタンを並べてトグルで選ぶ
const GRID: Record<
	number,
	{ cols: number; items: { v: number; label: string }[] }
> = {
	2: {
		cols: 7,
		items: Array.from({ length: 31 }, (_, i) => ({
			v: i + 1,
			label: String(i + 1),
		})),
	},
	3: {
		cols: 6,
		items: Array.from({ length: 12 }, (_, i) => ({
			v: i + 1,
			label: `${i + 1}月`,
		})),
	},
	4: {
		cols: 7,
		// 表示は月曜始まり
		items: [1, 2, 3, 4, 5, 6, 0].map((v) => ({ v, label: WEEKDAYS[v] })),
	},
};

const expr = $derived(buildCron(schedule));
const description = $derived(describeCron(schedule));

function reset() {
	schedule = defaultState();
	activeTab = "0";
}
</script>

<div class="cron-builder">
	<section class="cb-main" aria-label="スケジュールを設定">
		<PageTabs tabs={TABS} bind:active={activeTab} label="cron のフィールド" />

		<h4 class="cb-title">{spec.name}を選択</h4>

		<!-- [指定方法] [入力欄] を 1 行にまとめ、天気の地域指定と同じく選んだ欄が伸びて他が縮む。
		     毎回と 日・月・曜日のカスタムは入力欄を持たないので、プルダウンだけが伸びる。 -->
		<div class="cb-row">
			<div class="joined-field cb-joined" class:open={paneOpen}>
				<InlineSelect
					id="{id}-mode"
					class="cb-mode"
					ariaLabel="{spec.name}の指定方法"
					options={modes}
					bind:value={() => field.mode, setMode}
				/>
				<span class="divider" aria-hidden="true"></span>
				<div class="cb-pane" class:active={paneOpen && field.mode === "custom"} inert={!(paneOpen && field.mode === "custom")}>
					<input
						id="{id}-value"
						type="text"
						inputmode="numeric"
						autocomplete="off"
						class="cb-input"
						aria-label="{spec.name}を入力"
						placeholder="{spec.min}〜{spec.max}"
						bind:value={pickValue}
						onkeydown={onEnter(addValue)}
					/>
					<span class="cb-affix">{spec.name}</span>
				</div>
				<div class="cb-pane" class:active={field.mode === "interval"} inert={field.mode !== "interval"}>
					<span class="cb-affix cb-lead">毎</span>
					{#if typed}
						<input
							id="{id}-step"
							type="text"
							inputmode="numeric"
							autocomplete="off"
							class="cb-input"
							aria-label="間隔を入力"
							placeholder="2〜{spec.max}"
							bind:value={pickStep}
							onkeydown={onEnter(addInterval)}
						/>
						<span class="cb-affix">{stepUnit}ごと</span>
					{:else}
						<InlineSelect
							id="{id}-step"
							class="grow-select"
							ariaLabel="間隔を選択"
							options={stepOptions}
							bind:value={pickStep}
						/>
						<span class="cb-affix">ごと</span>
					{/if}
				</div>
				<div class="cb-pane" class:active={field.mode === "range"} inert={field.mode !== "range"}>
					{#if typed}
						<input
							id="{id}-range-start"
							type="text"
							inputmode="numeric"
							autocomplete="off"
							class="cb-input"
							aria-label="範囲の開始"
							placeholder={String(spec.min)}
							bind:value={pickStart}
							onkeydown={onEnter(addRange)}
						/>
						<span class="cb-affix">〜</span>
						<input
							id="{id}-range-end"
							type="text"
							inputmode="numeric"
							autocomplete="off"
							class="cb-input"
							aria-label="範囲の終了"
							placeholder={String(spec.max)}
							bind:value={pickEnd}
							onkeydown={onEnter(addRange)}
						/>
						<span class="cb-affix">{spec.name}</span>
						<span class="divider" aria-hidden="true"></span>
						<input
							id="{id}-range-step"
							type="text"
							inputmode="numeric"
							autocomplete="off"
							class="cb-input"
							aria-label="範囲内の間隔"
							placeholder="1"
							bind:value={pickRangeStep}
							onkeydown={onEnter(addRange)}
						/>
						<span class="cb-affix">{stepUnit}おき</span>
					{:else}
						<InlineSelect
							id="{id}-range-start"
							class="grow-select"
							ariaLabel="範囲の開始"
							options={valueOptions}
							bind:value={pickStart}
						/>
						<span class="cb-affix">〜</span>
						<InlineSelect
							id="{id}-range-end"
							class="grow-select"
							ariaLabel="範囲の終了"
							options={valueOptions}
							bind:value={pickEnd}
						/>
						<span class="divider" aria-hidden="true"></span>
						<InlineSelect
							id="{id}-range-step"
							class="grow-select"
							ariaLabel="範囲内の間隔"
							options={rangeStepOptions}
							bind:value={pickRangeStep}
						/>
					{/if}
				</div>
			</div>
			{#if paneOpen}
				<button
					type="button"
					class="cb-add"
					aria-label="追加"
					disabled={!canAdd}
					onclick={field.mode === "custom" ? addValue : field.mode === "interval" ? addInterval : addRange}
				>
					<span class="material-symbols-outlined" aria-hidden="true">add</span>
				</button>
			{/if}
		</div>

		{#if field.mode === "custom" && !typed}
			{@const grid = GRID[active]}
			<div
				class="cb-grid"
				style="--cols: {grid.cols}"
				transition:slide={{ duration: 250 }}
			>
				{#each grid.items as item (item.v)}
					<button
						type="button"
						class="cb-cell"
						class:selected={field.values.includes(item.v)}
						aria-pressed={field.values.includes(item.v)}
						aria-label={valueLabel(active, item.v)}
						onclick={() => toggleValue(item.v)}>{item.label}</button
					>
				{/each}
			</div>
		{:else if field.mode === "interval" && !inputError}
			<p class="cb-hint">{INTERVAL_HINTS[active]}</p>
		{:else if field.mode === "range" && !inputError}
			<p class="cb-hint">
				開始から終了まで、{typed ? "入力した" : "選んだ"}間隔で指定します。{active === 4
					? "曜日は日曜日から土曜日の順です。"
					: ""}
			</p>
		{/if}

		{#if inputError}
			<p class="cb-error" role="alert">{inputError}</p>
		{/if}

		{#if showChips}
			<div class="cb-chips">
				{#each chips as chip (chip.key)}
					<span class="cb-chip">
						{chip.label}
						<button
							type="button"
							aria-label="{chip.label}を削除"
							onclick={chip.remove}
						>
							<span class="material-symbols-outlined" aria-hidden="true">close</span>
						</button>
					</span>
				{:else}
					<span class="cb-muted">まだ選択されていません（{spec.everyLabel}）</span>
				{/each}
			</div>
		{/if}
	</section>

	<aside class="cb-summary" aria-label="生成された cron 式">
		<div class="cb-expression">
			<span class="cb-caption">CRON 式</span>
			<code>{expr}</code>
		</div>
		<dl class="cb-fields">
			{#each FIELDS as f, i (f.name)}
				<div class:current={i === active}>
					<dt>{f.name}</dt>
					<dd>{fieldExpr(schedule[i])}</dd>
				</div>
			{/each}
		</dl>
		<p class="cb-desc">{description}</p>
		{#if hasDomAndDow(schedule)}
			<p class="cb-notice">
				日と曜日を両方指定すると、どちらか一方が一致した日に実行されます。
			</p>
		{/if}
		<button type="button" class="cb-reset" onclick={reset}>設定をリセット</button>
	</aside>
</div>

<style>
	.cron-builder {
		display: grid;
		grid-template-columns: minmax(0, 1.5fr) minmax(220px, 1fr);
		gap: 20px;
		align-items: start;
	}
	.cb-main {
		min-width: 0;
	}
	/* PageTabs は横スクロール用に overflow-x: auto を持ち、タブの margin-bottom: -1px で
	   縦にもはみ出して縦スクロールバーが出るため、縦は切る */
	.cb-main :global(.page-tabs) {
		margin-top: 0;
		overflow-y: hidden;
	}
	.cb-title {
		margin: 4px 0 12px;
		font-size: 1rem;
		font-weight: 700;
		color: var(--text-primary);
	}
	.cb-hint {
		margin: 10px 0 0;
		font-size: 0.8rem;
		color: var(--text-secondary);
	}
	.cb-error {
		margin: 10px 0 0;
		font-size: 0.8rem;
		color: var(--color-error);
	}
	.cb-cell {
		padding: 8px 14px;
		border: 1px solid var(--border-divider);
		border-radius: var(--radius);
		background: var(--surface-1dp);
		color: var(--text-primary);
		font: inherit;
		font-size: 0.9rem;
		cursor: pointer;
		transition:
			border-color 0.15s ease,
			background-color 0.15s ease;
	}
	.cb-cell:hover {
		border-color: var(--text-secondary);
	}
	.cb-cell.selected {
		border-color: var(--color-primary);
		background: color-mix(in srgb, var(--color-primary) 16%, var(--surface-1dp));
		color: var(--text-high);
		font-weight: 700;
	}
	.cb-row {
		display: flex;
		align-items: stretch;
		gap: 8px;
	}
	/* 指定方法のプルダウン: 入力欄が無いときは行いっぱいに伸び、入力欄が開くと縮む
	   （定期報告の天気設定・報告先と同じ動き） */
	.cb-joined > :global(.cb-mode) {
		flex: 1 0 120px;
		transition: flex-grow 0.3s ease;
	}
	.cb-joined.open > :global(.cb-mode) {
		flex-grow: 0;
	}
	/* 共通の .joined-field .divider と違い、左右の余白なし */
	.cb-joined .divider {
		margin: 8px 0;
	}
	.cb-joined > .divider {
		transition: opacity 0.2s ease;
	}
	.cb-joined:not(.open) > .divider {
		opacity: 0;
	}
	.cb-pane {
		display: flex;
		flex: 0 1 0;
		width: 0;
		min-width: 0;
		overflow: hidden;
		opacity: 0;
		transition:
			flex-grow 0.3s ease,
			opacity 0.2s ease;
	}
	.cb-pane.active {
		flex-grow: 1;
		opacity: 1;
	}
	.cb-pane > :global(.grow-select) {
		flex: 1;
		min-width: 0;
	}
	.cb-affix.cb-lead {
		padding: 0 0 0 12px;
	}
	.joined-field {
		flex: 1;
	}
	.joined-field > :global(.grow-select) {
		flex: 1;
		min-width: 0;
	}
	/* 他の入力欄と同じく、フォーカス時も枠線・リングを出さない（input[type=text]:focus より強く当てる） */
	.cb-input,
	.cb-input:focus {
		flex: 1;
		width: 100%;
		min-width: 0;
		padding: 12px;
		border: 0;
		outline: none;
		box-shadow: none;
		background: transparent;
		color: var(--text-high);
		font: inherit;
		font-size: 0.95rem;
	}
	.cb-affix {
		align-self: center;
		padding: 0 4px 0 12px;
		color: var(--text-secondary);
		font-size: 0.9rem;
		white-space: nowrap;
	}
	.cb-affix:last-child {
		padding: 0 12px 0 0;
	}
	.cb-add {
		display: grid;
		place-items: center;
		width: 44px;
		flex-shrink: 0;
		border: 0;
		border-radius: 4px;
		background: var(--color-primary);
		color: var(--surface-0dp);
		cursor: pointer;
	}
	.cb-add:disabled {
		opacity: 0.4;
		cursor: not-allowed;
	}
	.cb-grid {
		margin: 12px 0 16px;
		display: grid;
		grid-template-columns: repeat(var(--cols), minmax(0, 1fr));
		gap: 6px;
	}
	.cb-cell {
		padding: 8px 0;
		text-align: center;
	}
	.cb-chips {
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
		margin-top: 16px;
	}
	.cb-chip {
		display: inline-flex;
		align-items: center;
		gap: 2px;
		padding: 4px 6px 4px 10px;
		border-radius: 999px;
		background: var(--surface-1dp);
		border: 1px solid var(--border-divider);
		font-size: 0.82rem;
		color: var(--text-primary);
	}
	.cb-chip button {
		display: grid;
		place-items: center;
		padding: 0;
		border: 0;
		background: transparent;
		color: var(--text-secondary);
		cursor: pointer;
	}
	.cb-chip .material-symbols-outlined {
		font-size: 16px;
	}
	.cb-muted {
		font-size: 0.8rem;
		color: var(--text-secondary);
	}

	/* 右: 生成された式 */
	.cb-summary {
		display: flex;
		flex-direction: column;
		gap: 12px;
		min-width: 0;
	}
	.cb-expression {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: 8px;
		padding: 14px;
		border-radius: var(--radius);
		background: var(--surface-1dp);
	}
	.cb-caption {
		font-size: 0.7rem;
		font-weight: 700;
		letter-spacing: 0.1em;
		color: var(--text-secondary);
	}
	.cb-expression code {
		font-family: var(--font-family-mono);
		font-size: 1.35rem;
		letter-spacing: 0.04em;
		color: var(--text-high);
		overflow-wrap: anywhere;
	}
	.cb-fields {
		display: grid;
		grid-template-columns: repeat(5, minmax(0, 1fr));
		gap: 4px;
		margin: 0;
		padding-bottom: 12px;
		border-bottom: 1px solid var(--border-divider);
		text-align: center;
	}
	.cb-fields dt {
		font-size: 0.7rem;
		color: var(--text-secondary);
	}
	.cb-fields dd {
		margin: 4px 0 0;
		font-family: var(--font-family-mono);
		font-size: 0.85rem;
		color: var(--text-primary);
		overflow-wrap: anywhere;
	}
	.cb-fields .current dt,
	.cb-fields .current dd {
		color: var(--color-primary);
		font-weight: 700;
	}
	.cb-desc {
		margin: 0;
		font-size: 0.88rem;
		line-height: 1.7;
		color: var(--text-primary);
	}
	.cb-notice {
		margin: 0;
		padding: 10px 12px;
		border-radius: var(--radius);
		background: color-mix(in srgb, #f5a623 14%, transparent);
		font-size: 0.78rem;
		line-height: 1.6;
		color: var(--text-primary);
	}
	.cb-reset {
		align-self: flex-start;
		padding: 0;
		border: 0;
		background: none;
		color: var(--text-secondary);
		font: inherit;
		font-size: 0.78rem;
		text-decoration: underline;
		cursor: pointer;
	}

	@media (max-width: 760px) {
		.cron-builder {
			grid-template-columns: 1fr;
		}
	}
</style>
