<script lang="ts">
// ─────────────────────────────────────────────────────────────────────────
// Playbook タブ（旧 app.js playbook 系 + index.html #tab-playbooks を移植）。
// 一度に出す量を減らすため、次の 3 つをページ内タブで切り替える:
//   - 手順書: Playbook 登録/編集（name が識別子。クリックで編集フォームへ読込）+ 検索
//   - 定期実行: スケジュール（cron プリセット→式）+ 一覧（有効/無効・削除）
//   - 実行履歴
// activeBot 変更で3本 fetch（list / schedules / runs）。playbookApi 使用（scope:'bot'）。
// ─────────────────────────────────────────────────────────────────────────

import { onDestroy } from "svelte";
import { ApiError } from "$lib/api/client";
import { playbookApi } from "$lib/api/services";
import type {
	PlaybookRecord,
	PlaybookRunRecord,
	PlaybookScheduleRecord,
} from "$lib/api/types";
import {
	Button,
	confirmDialog,
	EmptyState,
	Icon,
	PageTabs,
	Select,
} from "$lib/components/ui";
import { activeBot } from "$lib/stores/activeBot";
import { headerExtra } from "$lib/stores/headerExtra";
import { pushToast } from "$lib/stores/toast";
import CronField from "./delivery/CronField.svelte";
import {
	formatRunDuration,
	runStatusIcon,
	runStatusLabel,
} from "./playbooks/playbookUtils";

const VIEWS = [
	{ id: "list", label: "一覧", icon: "list_alt" },
	{ id: "schedules", label: "定期実行", icon: "event_repeat" },
	{ id: "runs", label: "実行履歴", icon: "history" },
];
let view = $state("list");
// 一覧タブ内で編集フォームを出している間 true
let editing = $state(false);

let playbooks = $state<PlaybookRecord[]>([]);
let schedules = $state<PlaybookScheduleRecord[]>([]);
let runs = $state<PlaybookRunRecord[]>([]);
// 実行履歴はサーバが新しい順に最大 50 件返すので、その範囲をクライアント側でページ分けする。
const RUNS_PAGE_SIZE = 5;
let runsPage = $state(0);
const runsTotalPages = $derived(
	Math.max(1, Math.ceil(runs.length / RUNS_PAGE_SIZE)),
);
const pagedRuns = $derived(
	runs.slice(runsPage * RUNS_PAGE_SIZE, (runsPage + 1) * RUNS_PAGE_SIZE),
);

// ── Playbook フォーム ──
let pbName = $state("");
let pbTitle = $state("");
let pbKeywords = $state("");
let pbDescription = $state("");
let pbSteps = $state("");
let searchQuery = $state("");

// ── スケジュール フォーム ──
let schPlaybookName = $state("");
const playbookOptions = $derived(
	playbooks.map((p) => ({ value: p.name, label: p.title })),
);
let cronExpression = $state("");
let schDescription = $state("");
let schEnabled = $state(true);
// 定期実行タブで追加フォームを出している間 true
let schAdding = $state(false);

function resetScheduleForm() {
	schPlaybookName = "";
	cronExpression = "";
	schDescription = "";
	schEnabled = true;
}

function reportError(e: unknown) {
	pushToast(
		e instanceof ApiError ? e.message : "エラーが発生しました",
		"error",
	);
}

async function loadPlaybooks(query?: string) {
	try {
		const res = await playbookApi.list(query ? { query } : undefined);
		playbooks = res.playbooks ?? [];
	} catch (e) {
		reportError(e);
		playbooks = [];
	}
}

async function loadSchedules() {
	try {
		const res = await playbookApi.schedules();
		schedules = res.schedules ?? [];
	} catch (e) {
		reportError(e);
		schedules = [];
	}
}

async function loadRuns() {
	try {
		const res = await playbookApi.runs();
		runs = res.runs ?? [];
		runsPage = 0;
	} catch (e) {
		reportError(e);
		runs = [];
	}
}

$effect(() => {
	void $activeBot?.id;
	void loadPlaybooks();
	void loadSchedules();
	void loadRuns();
});

function resetPlaybookForm() {
	pbName = "";
	pbTitle = "";
	pbKeywords = "";
	pbDescription = "";
	pbSteps = "";
}

function loadIntoForm(p: PlaybookRecord) {
	editing = true;
	pbName = p.name;
	pbTitle = p.title;
	pbKeywords = (p.keywords ?? []).join(", ");
	pbDescription = p.description ?? "";
	pbSteps = p.steps ?? "";
}

async function savePlaybook(e: SubmitEvent) {
	e.preventDefault();
	const name = pbName.trim();
	const title = pbTitle.trim();
	const steps = pbSteps.trim();
	if (!name || !title || !steps) return;
	const keywords = pbKeywords
		.split(",")
		.map((k) => k.trim())
		.filter((k) => k.length > 0);
	try {
		const res = await playbookApi.save({
			name,
			title,
			keywords,
			description: pbDescription.trim(),
			steps,
		});
		pushToast(res.message ?? "Playbookを保存しました。", "success");
		resetPlaybookForm();
		editing = false;
		await loadPlaybooks(searchQuery.trim() || undefined);
		await loadSchedules();
	} catch (err) {
		reportError(err);
	}
}

async function deletePlaybook(p: PlaybookRecord) {
	const ok = await confirmDialog({
		message: `本当にこのPlaybook「${p.title}」を削除しますか？`,
		danger: true,
		confirmLabel: "削除",
	});
	if (!ok) return;
	try {
		await playbookApi.delete(p.name);
		await loadPlaybooks(searchQuery.trim() || undefined);
		await loadSchedules();
	} catch (e) {
		reportError(e);
	}
}

async function runSearch() {
	await loadPlaybooks(searchQuery.trim() || undefined);
}

async function saveSchedule(e: SubmitEvent) {
	e.preventDefault();
	const playbookName = schPlaybookName.trim();
	const cron = cronExpression.trim();
	if (!playbookName || !cron) {
		pushToast("Playbookとcron式を入力してください。", "error");
		return;
	}
	try {
		const res = await playbookApi.saveSchedule({
			playbookName,
			cronExpression: cron,
			description: schDescription.trim(),
			enabled: schEnabled,
		});
		pushToast(res.message ?? "スケジュールを保存しました。", "success");
		resetScheduleForm();
		schAdding = false;
		await loadSchedules();
		await loadRuns();
	} catch (err) {
		reportError(err);
	}
}

async function toggleSchedule(s: PlaybookScheduleRecord) {
	const ok = await confirmDialog({
		message: s.enabled ? "無効にしますか？" : "有効にしますか？",
		confirmLabel: s.enabled ? "無効にする" : "有効にする",
	});
	if (!ok) return;
	try {
		await playbookApi.toggleSchedule({ id: s.id, enabled: !s.enabled });
		await loadSchedules();
	} catch (e) {
		reportError(e);
	}
}

async function deleteSchedule(s: PlaybookScheduleRecord) {
	const ok = await confirmDialog({
		message: `スケジュール「${s.playbook_name}」を削除しますか？`,
		danger: true,
		confirmLabel: "削除",
	});
	if (!ok) return;
	try {
		await playbookApi.deleteSchedule(s.id);
		await loadSchedules();
		await loadRuns();
	} catch (e) {
		reportError(e);
	}
}

// タブはシェルのヘッダーに出す（Discord ページと同じ）。
headerExtra.set(headerTabs);
onDestroy(() => headerExtra.set(null));
</script>

{#snippet headerTabs()}
	<PageTabs tabs={VIEWS} bind:active={view} label="Playbook の設定" flush />
{/snippet}

{#snippet playbookForm()}
	<div class="action-column card">
		<div class="column-header">
			<h3><Icon name="edit_note" class="header-icon-symbol" />Playbook の登録・編集</h3>
		</div>
		<form class="form-stack" onsubmit={savePlaybook}>
			<div class="form-row">
				<div class="form-group">
					<label for="playbook-name">プレイブック名 (英数字・ハイフン)<span class="required-mark"> *</span></label>
					<input
						type="text"
						id="playbook-name"
						required
						placeholder="例: daily-sales-check"
						pattern={"[a-zA-Z0-9\\-_\\/]+"}
						bind:value={pbName}
					/>
				</div>
				<div class="form-group">
					<label for="playbook-title">表示タイトル<span class="required-mark"> *</span></label>
					<input
						type="text"
						id="playbook-title"
						required
						placeholder="例: 毎日売上データ収集"
						bind:value={pbTitle}
					/>
				</div>
			</div>
			<div class="form-group">
				<label for="playbook-keywords">キーワード (カンマ区切り)</label>
				<input
					type="text"
					id="playbook-keywords"
					placeholder="例: 売上, スプレッドシート, 集計"
					bind:value={pbKeywords}
				/>
			</div>
			<div class="form-group">
				<label for="playbook-description">概要説明</label>
				<input
					type="text"
					id="playbook-description"
					placeholder="例: 特定サイトから前日の売上を自動転記する"
					bind:value={pbDescription}
				/>
			</div>
			<div class="form-group">
				<label for="playbook-steps">操作手順ステップ (Markdown形式等)<span class="required-mark"> *</span></label>
				<textarea
					id="playbook-steps"
					required
					class="pb-steps"
					placeholder={"例:\n1. http://example.com/login にアクセスする\n2. 資格情報を使ってログインする\n3. レポートをダウンロードする"}
					bind:value={pbSteps}
				></textarea>
			</div>
			<Button type="submit" variant="primary" block>プレイブックを保存</Button>
			{#if editing}
				<Button
					variant="secondary"
					block
					onclick={() => {
						editing = false;
						resetPlaybookForm();
					}}>キャンセル</Button
				>
			{/if}
		</form>
	</div>
{/snippet}

<section class="tab-view playbooks-tab">

	<!-- Playbook 一覧 -->
	{#if view === "list"}
	<div class="expense-actions-columns pb-grid pb-single">
		{#if editing}
			{@render playbookForm()}
		{:else}
		<div class="action-column card">
			<div class="pb-search-row">
			<div class="input-wrap">
				<input
					type="text"
					placeholder="キーワード・タイトルで検索..."
					bind:value={searchQuery}
				/>
				<button
					type="button"
					class="inline-button pb-search-button"
					title="検索"
					aria-label="検索"
					disabled={!searchQuery.trim()}
					onclick={runSearch}><Icon name="search" /></button
				>
			</div>
				<Button
					variant="primary"
					onclick={() => {
						resetPlaybookForm();
						editing = true;
					}}><Icon name="add" />追加</Button
				>
			</div>

			<div class="entry-list">
				{#if playbooks.length > 0}
					{#each playbooks as p (p.name)}
						<div class="card-item glass entry">
							<div class="entry-body">
								<div class="entry-title">
									<span class="entry-name">{p.title}</span>
									<span class="pb-name-badge">{p.name}</span>
								</div>
								{#if p.description}
									<div class="pb-desc">{p.description}</div>
								{/if}
								{#if p.keywords.length > 0}
									<div class="pb-keywords">
										{#each p.keywords as kw (kw)}
											<span class="badge">{kw}</span>
										{/each}
									</div>
								{/if}
							</div>
							<div class="entry-actions">
								<button
									type="button"
									class="icon-plain"
									aria-label="編集"
									onclick={() => loadIntoForm(p)}
								>
									<Icon name="edit" />
								</button>
								<button
									type="button"
									class="icon-plain danger"
									aria-label="削除"
									onclick={() => deletePlaybook(p)}
								>
									<Icon name="delete" />
								</button>
							</div>
						</div>
					{/each}
				{:else}
					<EmptyState icon="list_alt" message="Playbookが登録されていません。" />
				{/if}
			</div>
		</div>
		{/if}
	</div>
	{/if}

	<!-- スケジュール -->
	{#if view === "schedules"}
	<div class="expense-actions-columns pb-grid pb-single">
		{#if schAdding}
		<!-- スケジュール フォーム -->
		<div class="action-column card">
			<div class="column-header">
				<h3><Icon name="schedule" class="header-icon-symbol" />定期実行スケジュール設定</h3>
			</div>
			<form class="form-stack" onsubmit={saveSchedule}>
				<div class="form-group">
					<label for="schedule-playbook-select">実行するPlaybook<span class="required-mark"> *</span></label>
					<Select
						id="schedule-playbook-select"
						options={playbookOptions}
						placeholder="Playbookを選択"
						bind:value={schPlaybookName}
					/>
				</div>
				<div class="form-group">
					<CronField
						id="schedule-cron-expression"
						label="実行スケジュール"
						required
						placeholder="例: 0 9 * * * (毎朝9時)"
						bind:value={cronExpression}
					/>
				</div>
				<div class="form-group">
					<label for="schedule-description">メモ</label>
					<input
						type="text"
						id="schedule-description"
						placeholder="例: 毎朝の売上チェック"
						bind:value={schDescription}
					/>
				</div>
				<label class="check-row">
					<input type="checkbox" class="checkbox-custom" bind:checked={schEnabled} />
					<span>有効</span>
				</label>
				<Button type="submit" variant="primary" block>スケジュールを保存</Button>
				<Button
					variant="secondary"
					block
					onclick={() => {
						schAdding = false;
						resetScheduleForm();
					}}>キャンセル</Button
				>
			</form>
		</div>
		{:else}
		<!-- スケジュール一覧 -->
		<div class="action-column card">
			<div class="column-header actions-end">
				<Button
					variant="primary"
					onclick={() => {
						resetScheduleForm();
						schAdding = true;
					}}><Icon name="add" />追加</Button
				>
			</div>
			<div class="entry-list">
				{#if schedules.length > 0}
					{#each schedules as s (s.id)}
						<div class="card-item glass entry">
							<div class="entry-body">
								<div class="entry-title">
									<span class="entry-name">{s.playbook_name}</span>
								</div>
								<div class="pb-cron">{s.cron_expression}</div>
								{#if s.description}
									<div class="pb-desc">{s.description}</div>
								{/if}
								{#if s.last_run_at}
									<div class="pb-lastrun">前回: {s.last_run_at}</div>
								{/if}
							</div>
							<div class="entry-actions">
								<button
									type="button"
									class="btn-mini"
									class:is-on={s.enabled}
									onclick={() => toggleSchedule(s)}
								>
									{s.enabled ? "有効" : "無効"}
								</button>
								<button
									type="button"
									class="icon-plain danger"
									aria-label="削除"
									onclick={() => deleteSchedule(s)}
								>
									<Icon name="delete" />
								</button>
							</div>
						</div>
					{/each}
				{:else}
					<EmptyState
						icon="event_repeat"
						message="スケジュールが登録されていません。"
					/>
				{/if}
			</div>
		</div>
		{/if}
	</div>

	{/if}

	<!-- 実行履歴 -->
	{#if view === "runs"}
	<div class="card">
		<div class="run-list">
			{#if runs.length > 0}
				{#each pagedRuns as run (run.id)}
					<div
						class="run-entry"
						class:failed={run.status === "failed"}
						class:success={run.status === "success"}
					>
						<div class="pb-run-head">
							<Icon name={runStatusIcon(run.status)} />
							<span class="pb-run-name">{run.playbook_name}</span>
							<span class="pb-run-status">{runStatusLabel(run.status)}</span>
						</div>
						<div class="pb-run-meta">
							{run.started_at}{#if formatRunDuration(run)}
								・所要 {formatRunDuration(run)}{/if}
						</div>
						{#if run.output}
							<pre class="pb-run-output">{run.output.slice(0, 300)}</pre>
						{/if}
					</div>
				{/each}
				{#if runsTotalPages > 1}
					<div class="pb-pager">
						<Button
							variant="secondary"
							small
							disabled={runsPage === 0}
							onclick={() => (runsPage -= 1)}>前へ</Button
						>
						<span class="pb-pager-info">{runsPage + 1} / {runsTotalPages}</span>
						<Button
							variant="secondary"
							small
							disabled={runsPage + 1 >= runsTotalPages}
							onclick={() => (runsPage += 1)}>次へ</Button
						>
					</div>
				{/if}
			{:else}
				<EmptyState icon="history" message="実行履歴がありません。" />
			{/if}
		</div>
	</div>
	{/if}
</section>

<style>
	.pb-grid {
		grid-template-columns: repeat(2, minmax(0, 1fr));
	}
	.pb-grid.pb-single {
		grid-template-columns: minmax(0, 1fr);
	}
	@media (max-width: 768px) {
		.pb-grid {
			grid-template-columns: minmax(0, 1fr);
		}
	}
	.pb-steps {
		min-height: 150px;
		font-family: var(--font-family-mono);
		font-size: 0.8rem;
	}
	.pb-pager {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 12px;
		margin-top: 8px;
	}
	.pb-pager-info {
		font-size: 0.85rem;
		color: var(--text-secondary);
	}
	.pb-search-row {
		display: flex;
		align-items: center;
		gap: 8px;
		margin-bottom: 12px;
	}
	/* 検索ボタンは貼り付けボタンと違いアクセント色 */
	.pb-search-button {
		color: var(--color-primary);
	}
	.pb-search-button:disabled {
		color: var(--text-secondary);
		opacity: 0.4;
		cursor: default;
	}
	.pb-name-badge {
		font-size: 0.72rem;
		font-family: var(--font-family-mono);
		color: var(--color-primary);
	}
	.pb-desc {
		font-size: 0.8rem;
		color: var(--color-zinc-muted);
		margin-top: 4px;
	}
	.pb-cron {
		font-family: var(--font-family-mono);
		font-size: 0.8rem;
		color: var(--color-zinc-muted);
		margin-top: 4px;
	}
	.pb-lastrun {
		font-size: 0.72rem;
		color: var(--color-zinc-muted);
		margin-top: 2px;
	}
	.pb-keywords {
		display: flex;
		flex-wrap: wrap;
		gap: 4px;
		margin-top: 6px;
	}
	.pb-run-head {
		display: flex;
		align-items: center;
		gap: 6px;
	}
	.pb-run-name {
		font-weight: 600;
		font-size: 0.85rem;
	}
	.pb-run-status {
		font-size: 0.72rem;
		color: var(--color-zinc-muted);
	}
	.pb-run-meta {
		font-size: 0.72rem;
		color: var(--color-zinc-muted);
		margin-top: 2px;
	}
	.pb-run-output {
		white-space: pre-wrap;
		word-break: break-word;
		max-height: 80px;
		overflow-y: auto;
		font-size: 0.72rem;
		margin: 6px 0 0;
		font-family: var(--font-family-mono);
		color: var(--color-zinc-muted);
	}
</style>
