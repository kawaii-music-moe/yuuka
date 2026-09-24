<script lang="ts">
// ─────────────────────────────────────────────────────────────────────────
// Playbook タブ（旧 app.js playbook 系 + index.html #tab-playbooks を移植）。
//   - Playbook 登録/編集（name が識別子。クリックで編集フォームへ読込）+ 検索
//   - 定期実行スケジュール（cron プリセット→式）+ 一覧（有効/無効・削除）
//   - 実行履歴
// activeBot 変更で3本 fetch（list / schedules / runs）。playbookApi 使用（scope:'bot'）。
// ─────────────────────────────────────────────────────────────────────────

import { ApiError } from "$lib/api/client";
import { playbookApi } from "$lib/api/services";
import type {
	PlaybookRecord,
	PlaybookRunRecord,
	PlaybookScheduleRecord,
} from "$lib/api/types";
import {
	Badge,
	Button,
	confirmDialog,
	EmptyState,
	Icon,
} from "$lib/components/ui";
import { activeBot } from "$lib/stores/activeBot";
import { pushToast } from "$lib/stores/toast";
import {
	formatRunDuration,
	runStatusIcon,
	runStatusLabel,
} from "./playbooks/playbookUtils";

let playbooks = $state<PlaybookRecord[]>([]);
let schedules = $state<PlaybookScheduleRecord[]>([]);
let runs = $state<PlaybookRunRecord[]>([]);

// ── Playbook フォーム ──
let pbName = $state("");
let pbTitle = $state("");
let pbKeywords = $state("");
let pbDescription = $state("");
let pbSteps = $state("");
let searchQuery = $state("");

// ── スケジュール フォーム ──
let schPlaybookName = $state("");
let cronPreset = $state("");
let cronExpression = $state("");
let schDescription = $state("");
let schEnabled = $state(true);

const CRON_PRESETS: { value: string; label: string }[] = [
	{ value: "0 9 * * *", label: "毎朝9時" },
	{ value: "0 18 * * *", label: "毎夕18時" },
	{ value: "0 9 * * 1", label: "毎週月曜 9時" },
	{ value: "0 9 1 * *", label: "毎月1日 9時" },
	{ value: "*/30 * * * *", label: "30分ごと" },
	{ value: "0 * * * *", label: "1時間ごと" },
];

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

// cron プリセット選択で式を反映（custom はそのまま手入力）。
function onCronPreset(e: Event) {
	const v = (e.currentTarget as HTMLSelectElement).value;
	cronPreset = v;
	if (v && v !== "custom") cronExpression = v;
}

function resetPlaybookForm() {
	pbName = "";
	pbTitle = "";
	pbKeywords = "";
	pbDescription = "";
	pbSteps = "";
}

function loadIntoForm(p: PlaybookRecord) {
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
		schPlaybookName = "";
		cronPreset = "";
		cronExpression = "";
		schDescription = "";
		schEnabled = true;
		await loadSchedules();
		await loadRuns();
	} catch (err) {
		reportError(err);
	}
}

async function toggleSchedule(s: PlaybookScheduleRecord) {
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
</script>

<section class="tab-view">
	<div class="expense-actions-columns">
		<!-- Playbook フォーム -->
		<div class="action-column card">
			<div class="column-header">
				<h3><Icon name="edit_note" class="header-icon-symbol" />Playbook の登録・編集</h3>
			</div>
			<p class="description-text">
				AIエージェントに実行させたい操作手順（ブラウザ操作など）のプレイブックを登録・更新します。
			</p>
			<form class="pb-form" onsubmit={savePlaybook}>
				<div class="form-row">
					<div class="form-group">
						<label for="playbook-name">プレイブック名 (英数字・ハイフン) *</label>
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
						<label for="playbook-title">表示タイトル *</label>
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
					<label for="playbook-steps">操作手順ステップ (Markdown形式等) *</label>
					<textarea
						id="playbook-steps"
						required
						class="pb-steps"
						placeholder={"例:\n1. http://example.com/login にアクセスする\n2. 資格情報を使ってログインする\n3. レポートをダウンロードする"}
						bind:value={pbSteps}
					></textarea>
				</div>
				<Button type="submit" variant="primary" block>プレイブックを保存</Button>
			</form>
		</div>

		<!-- Playbook 一覧 -->
		<div class="action-column card">
			<div class="column-header">
				<h3><Icon name="list_alt" class="header-icon-symbol" />登録済み Playbook 一覧</h3>
			</div>
			<p class="description-text">現在このBotに登録されている自動化手順書です。</p>

			<div class="pb-search">
				<input
					type="text"
					placeholder="キーワード・タイトルで検索..."
					bind:value={searchQuery}
				/>
				<Button variant="secondary" onclick={runSearch}>検索</Button>
			</div>

			<div class="pb-list">
				{#if playbooks.length > 0}
					{#each playbooks as p (p.name)}
						<div class="card-item glass pb-item">
							<div class="pb-item-body">
								<div class="pb-item-title">
									<span class="pb-title">{p.title}</span>
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
							<div class="pb-item-actions">
								<button type="button" class="btn-mini" onclick={() => loadIntoForm(p)}>
									<Icon name="edit" />編集
								</button>
								<button
									type="button"
									class="btn-trash"
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
	</div>

	<!-- スケジュール -->
	<div class="expense-actions-columns pb-section-spacer">
		<!-- スケジュール フォーム -->
		<div class="action-column card">
			<div class="column-header">
				<h3><Icon name="schedule" class="header-icon-symbol" />定期実行スケジュール設定</h3>
			</div>
			<p class="description-text">
				Playbookを定期的に自動実行するスケジュールを設定します。
			</p>
			<form class="pb-form" onsubmit={saveSchedule}>
				<div class="form-group">
					<label for="schedule-playbook-select">実行するPlaybook *</label>
					<select id="schedule-playbook-select" required bind:value={schPlaybookName}>
						<option value="">-- Playbookを選択 --</option>
						{#each playbooks as p (p.name)}
							<option value={p.name}>{p.title}</option>
						{/each}
					</select>
				</div>
				<div class="form-group">
					<label for="schedule-cron-preset">実行頻度プリセット</label>
					<select id="schedule-cron-preset" value={cronPreset} onchange={onCronPreset}>
						<option value="">-- プリセットを選択 --</option>
						{#each CRON_PRESETS as preset (preset.value)}
							<option value={preset.value}>{preset.label}</option>
						{/each}
						<option value="custom">カスタム入力</option>
					</select>
				</div>
				<div class="form-group">
					<label for="schedule-cron-expression">Cron式 *</label>
					<input
						type="text"
						id="schedule-cron-expression"
						required
						class="pb-mono"
						placeholder="例: 0 9 * * * (毎朝9時)"
						bind:value={cronExpression}
					/>
					<span class="field-sub">
						分 時 日 月 曜日 の順。例: <code>0 9 * * 1</code> = 毎週月曜9時
					</span>
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
				<label class="pb-enabled-row">
					<input type="checkbox" class="checkbox-custom" bind:checked={schEnabled} />
					<span>有効</span>
				</label>
				<Button type="submit" variant="primary" block>スケジュールを保存</Button>
			</form>
		</div>

		<!-- スケジュール一覧 -->
		<div class="action-column card">
			<div class="column-header">
				<h3><Icon name="event_repeat" class="header-icon-symbol" />登録済みスケジュール一覧</h3>
			</div>
			<p class="description-text">設定されている定期実行スケジュールです。</p>
			<div class="pb-list">
				{#if schedules.length > 0}
					{#each schedules as s (s.id)}
						<div class="card-item glass pb-item">
							<div class="pb-item-body">
								<div class="pb-item-title">
									<span class="pb-title">{s.playbook_name}</span>
									{#if s.enabled}
										<Badge tone="status-active">有効</Badge>
									{:else}
										<Badge>停止中</Badge>
									{/if}
								</div>
								<div class="pb-cron">{s.cron_expression}</div>
								{#if s.description}
									<div class="pb-desc">{s.description}</div>
								{/if}
								{#if s.last_run_at}
									<div class="pb-lastrun">前回: {s.last_run_at}</div>
								{/if}
							</div>
							<div class="pb-item-actions">
								<button type="button" class="btn-mini" onclick={() => toggleSchedule(s)}>
									{s.enabled ? "無効化" : "有効化"}
								</button>
								<button
									type="button"
									class="btn-trash"
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
	</div>

	<!-- 実行履歴 -->
	<div class="card pb-section-spacer">
		<div class="column-header">
			<h3><Icon name="history" class="header-icon-symbol" />実行履歴</h3>
		</div>
		<p class="description-text">Playbookの自動実行ログです。</p>
		<div class="pb-runs">
			{#if runs.length > 0}
				{#each runs as run (run.id)}
					<div
						class="pb-run"
						class:pb-run-failed={run.status === "failed"}
						class:pb-run-success={run.status === "success"}
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
			{:else}
				<EmptyState icon="history" message="実行履歴がありません。" />
			{/if}
		</div>
	</div>
</section>

<style>
	.pb-form {
		display: flex;
		flex-direction: column;
		gap: 12px;
		margin-top: 8px;
	}
	.pb-steps {
		min-height: 150px;
		font-family: var(--font-family-mono);
		font-size: 0.8rem;
	}
	.pb-mono {
		font-family: var(--font-family-mono);
	}
	.pb-search {
		display: flex;
		gap: 8px;
		margin-bottom: 12px;
	}
	.pb-search input {
		flex-grow: 1;
	}
	.pb-list {
		display: flex;
		flex-direction: column;
		gap: 12px;
		max-height: 480px;
		overflow-y: auto;
		padding-right: 4px;
	}
	.pb-item {
		display: flex;
		justify-content: space-between;
		gap: 10px;
		align-items: flex-start;
	}
	.pb-item-body {
		flex: 1;
		min-width: 0;
	}
	.pb-item-title {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
	}
	.pb-title {
		font-weight: 600;
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
	.pb-item-actions {
		display: flex;
		gap: 6px;
		align-items: center;
		flex-shrink: 0;
	}
	.pb-enabled-row {
		display: flex;
		align-items: center;
		gap: 10px;
		font-size: 0.85rem;
	}
	.pb-section-spacer {
		margin-top: 24px;
	}
	.pb-runs {
		display: flex;
		flex-direction: column;
		gap: 8px;
		max-height: 360px;
		overflow-y: auto;
		padding-right: 4px;
		margin-top: 8px;
	}
	.pb-run {
		border-left: 3px solid var(--color-zinc-muted);
		padding: 8px 12px;
		border-radius: var(--radius);
		background: var(--surface-1dp);
	}
	.pb-run-success {
		border-left-color: var(--color-green, #22c55e);
	}
	.pb-run-failed {
		border-left-color: var(--color-red, #ef4444);
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
