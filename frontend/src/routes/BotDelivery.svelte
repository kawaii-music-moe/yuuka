<script lang="ts">
// ─────────────────────────────────────────────────────────────────────────
// 定期報告タブ。天気・ニュース・タスクなどから載せる内容を選んで、名前を付けた報告を定時配信する。
// deliveryApi 使用（scope:'bot'）。
//   - 上: 登録・編集フォーム（名前 / スケジュール + 報告先 / 天気設定 / ニュースRSS / そのほかの内容）
//   - 中: 登録済みの一覧（名前・編集・有効/無効・削除）
//   - 下: テスト配信（登録済みから選んで送る）
// タスク・予定・支払い予定・家計（🔒）を選ぶと、報告先は DM だけになる（チャンネルは選べない）。
// bot-scoped のため activeBot 切替に $effect で追従する。
// ─────────────────────────────────────────────────────────────────────────

import { ApiError } from "$lib/api/client";
import { deliveryApi } from "$lib/api/services";
import type {
	PeriodicReport,
	PeriodicReportInput,
	ReportSection,
} from "$lib/api/types";
import {
	Button,
	Checkbox,
	confirmDialog,
	EmptyState,
	Icon,
	Select,
} from "$lib/components/ui";
import { activeBot } from "$lib/stores/activeBot";
import { pushToast } from "$lib/stores/toast";
import CronField from "./delivery/CronField.svelte";
import HelpTip from "./delivery/HelpTip.svelte";
import TargetField from "./delivery/TargetField.svelte";

// ── 載せる内容 ──
// 並びは保存・表示の順。天気・ニュースは各設定欄の見出しに、残りは「そのほかの内容」に置く。
const SECTION_ORDER: ReportSection[] = [
	"weather",
	"news",
	"tasks",
	"schedules",
	"payments",
	"finance",
	"topics",
];
/** 本人だけが見るべき内容。1 つでも選ぶと DM にだけ送る */
const PRIVATE_SECTIONS: ReportSection[] = [
	"tasks",
	"schedules",
	"payments",
	"finance",
];
const OTHER_SECTIONS: { value: ReportSection; label: string }[] = [
	{ value: "tasks", label: "タスク（完了・持ち越し）" },
	{ value: "schedules", label: "これからの予定" },
	{ value: "payments", label: "支払い予定" },
	{ value: "finance", label: "家計（収入・支出）" },
	{ value: "topics", label: "会話のトピック" },
];
/** 一覧に出す短い名前 */
const SECTION_SHORT: Record<ReportSection, string> = {
	weather: "天気",
	news: "ニュース",
	tasks: "タスク",
	schedules: "予定",
	payments: "支払い予定",
	finance: "家計",
	topics: "会話トピック",
};

function noSections(): Record<ReportSection, boolean> {
	return Object.fromEntries(SECTION_ORDER.map((s) => [s, false])) as Record<
		ReportSection,
		boolean
	>;
}

// ── 登録・編集フォーム state ──
let editingId = $state<number | null>(null);
let fName = $state("");
let fCron = $state("");
let fTargetType = $state<"dm" | "channel">("dm");
let fTargetId = $state("");
let fSections = $state<Record<ReportSection, boolean>>(noSections());
// 天気予報の地域は地点名か緯度経度のどちらで指定するか
type RegionMode = "name" | "coords";
const REGION_OPTIONS: { value: RegionMode; label: string }[] = [
	{ value: "name", label: "地点名" },
	{ value: "coords", label: "緯度経度" },
];
let regionMode = $state<RegionMode>("name");
let fLocation = $state("");
let fLat = $state("");
let fLng = $state("");
let feeds = $state<string[]>([]);
let feedInput = $state("");
let fKeywords = $state("");
let saving = $state(false);
let formEl: HTMLFormElement | undefined = $state();

const hasPrivate = $derived(PRIVATE_SECTIONS.some((s) => fSections[s]));
const pickedSections = $derived(SECTION_ORDER.filter((s) => fSections[s]));
// 非公開の内容を選んだらチャンネルは選べない
const targetOptions = $derived<
	{ value: "dm" | "channel"; label: string; disabled?: boolean }[]
>([
	{ value: "dm", label: "DM" },
	{ value: "channel", label: "チャンネル", disabled: hasPrivate },
]);
$effect(() => {
	if (hasPrivate) fTargetType = "dm";
});
const saveDisabled = $derived(
	saving || !fName.trim() || !fCron.trim() || pickedSections.length === 0,
);

// ── 一覧・テスト配信 state ──
let reports = $state<PeriodicReport[]>([]);
let testId = $state("");
let testing = $state(false);
const testOptions = $derived(
	reports.map((r) => ({ value: String(r.id), label: r.name })),
);

function reportError(e: unknown) {
	pushToast(
		e instanceof ApiError ? e.message : "エラーが発生しました",
		"error",
	);
}

function resetForm() {
	editingId = null;
	fName = "";
	fCron = "";
	fTargetType = "dm";
	fTargetId = "";
	fSections = noSections();
	regionMode = "name";
	fLocation = "";
	fLat = "";
	fLng = "";
	feeds = [];
	feedInput = "";
	fKeywords = "";
}

function editReport(r: PeriodicReport) {
	editingId = r.id;
	fName = r.name;
	fCron = r.schedule_cron;
	fTargetType = r.target_type;
	fTargetId = r.target_id ?? "";
	fSections = Object.fromEntries(
		SECTION_ORDER.map((s) => [s, r.sections.includes(s)]),
	) as Record<ReportSection, boolean>;
	fLat = r.weather_lat != null ? String(r.weather_lat) : "";
	fLng = r.weather_lng != null ? String(r.weather_lng) : "";
	regionMode = fLat !== "" || fLng !== "" ? "coords" : "name";
	fLocation = r.location_name ?? "";
	feeds = r.news_feeds.slice();
	feedInput = "";
	fKeywords = r.news_keywords.join(", ");
	formEl?.scrollIntoView({ behavior: "smooth", block: "start" });
}

async function loadReports() {
	try {
		const res = await deliveryApi.list();
		reports = res.reports ?? [];
		if (!reports.some((r) => String(r.id) === testId)) {
			testId = reports.length > 0 ? String(reports[0].id) : "";
		}
	} catch (e) {
		reportError(e);
	}
}

function addFeed() {
	const url = feedInput.trim();
	if (!url) return;
	if (feeds.includes(url)) {
		pushToast("同じフィードが既に登録されています。", "warning");
		return;
	}
	feeds = [...feeds, url];
	feedInput = "";
}

async function pasteFeed() {
	try {
		feedInput = (await navigator.clipboard.readText()).trim();
	} catch {
		pushToast("クリップボードを読み取れませんでした。", "error");
	}
}

function removeFeed(idx: number) {
	feeds = feeds.filter((_, i) => i !== idx);
}

async function saveReport(e: SubmitEvent) {
	e.preventDefault();
	saving = true;
	try {
		const channel = !hasPrivate && fTargetType === "channel";
		const payload: PeriodicReportInput = {
			name: fName.trim(),
			schedule_cron: fCron.trim(),
			target_type: channel ? "channel" : "dm",
			target_id: channel ? fTargetId.trim() : null,
			sections: pickedSections,
			// 地点は選んだ指定方法の値だけを送る（緯度経度があると地点名より優先されるため）
			location_name: regionMode === "name" ? fLocation.trim() || null : null,
			weather_lat: regionMode === "coords" && fLat !== "" ? Number(fLat) : null,
			weather_lng: regionMode === "coords" && fLng !== "" ? Number(fLng) : null,
			news_feeds: feeds,
			news_keywords: fKeywords
				.split(",")
				.map((k) => k.trim())
				.filter((k) => k.length > 0),
		};
		const res =
			editingId == null
				? await deliveryApi.create(payload)
				: await deliveryApi.update(editingId, payload);
		pushToast(res.message ?? "保存しました。", "success");
		resetForm();
		await loadReports();
	} catch (e) {
		reportError(e);
	} finally {
		saving = false;
	}
}

async function toggleEnabled(r: PeriodicReport) {
	try {
		await deliveryApi.update(r.id, { enabled: !r.enabled });
		await loadReports();
	} catch (e) {
		reportError(e);
	}
}

async function deleteReport(r: PeriodicReport) {
	const ok = await confirmDialog({
		message: `定期報告「${r.name}」を削除しますか？`,
		danger: true,
		confirmLabel: "削除",
	});
	if (!ok) return;
	try {
		const res = await deliveryApi.delete(r.id);
		pushToast(res.message ?? "削除しました。", "success");
		if (editingId === r.id) resetForm();
		await loadReports();
	} catch (e) {
		reportError(e);
	}
}

async function runTest() {
	if (!testId) return;
	testing = true;
	try {
		const res = await deliveryApi.test(Number(testId));
		pushToast(res.message ?? "テスト配信しました。", "success");
	} catch (e) {
		reportError(e);
	} finally {
		testing = false;
	}
}

function describeTarget(r: PeriodicReport): string {
	return r.target_type === "channel" && !r.private ? "チャンネル" : "DM";
}

// activeBot 切替に追従して一覧を再取得し、フォームを空に戻す
$effect(() => {
	void $activeBot?.id;
	resetForm();
	loadReports();
});
</script>

<section class="tab-view delivery-tab">
	<!-- 登録・編集 -->
	<div class="card">
		<div class="column-header briefing-header">
			<h3>
				<span class="material-symbols-outlined header-icon-symbol">{editingId == null ? "edit_note" : "edit"}</span
				>{editingId == null ? "定期報告の登録" : "定期報告の編集"}
			</h3>
			<HelpTip
				id="report-help"
				label="定期報告で配信される内容"
				align="end"
			>
				選んだ内容をまとめて、スケジュールどおりに Discord へ配信します。タスク・家計・会話のトピックは前回の配信予定時刻から今まで、予定と支払い予定は次回の配信予定時刻までの分を載せます。<span class="lock-inline"><Icon name="lock" size={14} /></span>
				の付いた内容を選ぶと DM にだけ送ります。会話のトピックは AI でまとめたときだけ載ります。
			</HelpTip>
		</div>
		<form class="report-form" bind:this={formEl} onsubmit={saveReport}>
			<div class="form-group">
				<label for="report-name">名前</label>
				<input
					id="report-name"
					type="text"
					autocomplete="off"
					maxlength="40"
					placeholder="例: 朝報、日報、週報"
					bind:value={fName}
				/>
			</div>
			<div class="schedule-row">
				<div class="form-group cron-group">
					<CronField id="report-cron" placeholder="0 7 * * * (毎朝7時)" bind:value={fCron} />
				</div>
				<div class="form-group target-group">
					<label for="report-target-type">報告先</label>
					<TargetField
						id="report-target-type"
						idInputId="report-target-id"
						options={targetOptions}
						bind:type={fTargetType}
						bind:targetId={fTargetId}
					/>
					{#if hasPrivate}
						<p class="field-hint">
							<span class="lock-inline"><Icon name="lock" size={14} /></span>
							の内容を含むため DM にだけ送ります。
						</p>
					{/if}
				</div>
			</div>

			<!-- 天気設定 -->
			<div class="form-group content-block">
				<span class="block-title">天気設定</span>
				<Checkbox bind:checked={fSections.weather} label="天気予報を載せる" />
				<div class="block-body" class:off={!fSections.weather} inert={!fSections.weather}>
					<div class="joined-field">
						<Select
							id="report-region-mode"
							ariaLabel="天気予報の地域の指定方法"
							options={REGION_OPTIONS}
							bind:value={regionMode}
						/>
						<span class="divider" aria-hidden="true"></span>
						<div class="region-pane" class:active={regionMode === "name"} inert={regionMode !== "name"}>
							<input
								type="text"
								autocomplete="off"
								placeholder="地点名（例: 東京）"
								aria-label="地点名"
								bind:value={fLocation}
							/>
						</div>
						<div class="region-pane" class:active={regionMode === "coords"} inert={regionMode !== "coords"}>
							<input
								type="number"
								step="any"
								placeholder="緯度（例: 35.68）"
								aria-label="緯度"
								bind:value={fLat}
							/>
							<span class="divider" aria-hidden="true"></span>
							<input
								type="number"
								step="any"
								placeholder="経度（例: 139.76）"
								aria-label="経度"
								bind:value={fLng}
							/>
						</div>
					</div>
				</div>
			</div>

			<!-- ニュースRSS -->
			<div class="form-group content-block">
				<span class="block-title">ニュースRSS</span>
				<Checkbox bind:checked={fSections.news} label="ニュースを載せる" />
				<div class="block-body" class:off={!fSections.news} inert={!fSections.news}>
					<div class="feed-row">
						<div class="input-wrap feed-input-wrap">
							<input
								id="report-feed-input"
								type="url"
								autocomplete="off"
								placeholder="https://example.com/rss.xml"
								aria-label="RSSフィードのURL"
								class="feed-input"
								bind:value={feedInput}
								onkeydown={(e) => {
									if (e.key === "Enter") {
										e.preventDefault();
										addFeed();
									}
								}}
							/>
							<button
								type="button"
								class="inline-button"
								onclick={pasteFeed}
								aria-label="貼り付け"
								title="貼り付け"
							>
								<span class="material-symbols-outlined" aria-hidden="true">content_paste</span>
							</button>
						</div>
						<button
							type="button"
							class="icon-action"
							onclick={addFeed}
							disabled={!feedInput.trim()}
							aria-label="フィードを追加"
							title="フィードを追加"
						>
							<span class="material-symbols-outlined" aria-hidden="true">add</span>
						</button>
					</div>
					{#if feeds.length === 0}
						<p class="feed-empty">フィードは登録されていません。</p>
					{:else}
						<details class="feed-list" open>
							<summary>登録済みフィード（{feeds.length}件）</summary>
							<ul>
								{#each feeds as feed, idx (feed)}
									<li>
										<span class="feed-url" title={feed}>{feed}</span>
										<button
											type="button"
											class="feed-delete"
											aria-label="フィードを削除"
											title="削除"
											onclick={() => removeFeed(idx)}
										>
											<Icon name="delete" size={18} />
										</button>
									</li>
								{/each}
							</ul>
						</details>
					{/if}
					<input
						class="keyword-input"
						type="text"
						autocomplete="off"
						placeholder="絞り込みキーワード（カンマ区切り・任意）例: AI, 天文, ゲーム"
						aria-label="ニュース絞り込みキーワード"
						bind:value={fKeywords}
					/>
				</div>
			</div>

			<!-- そのほかの内容 -->
			<div class="form-group content-block">
				<span class="block-title">そのほかの内容</span>
				<div class="section-options">
					{#each OTHER_SECTIONS as s (s.value)}
						<button
							type="button"
							class="section-option"
							class:active={fSections[s.value]}
							aria-pressed={fSections[s.value]}
							onclick={() => (fSections[s.value] = !fSections[s.value])}
						>
							{s.label}
							{#if PRIVATE_SECTIONS.includes(s.value)}
								<span class="lock" title="DM にだけ送ります" aria-label="DM のみ"><Icon name="lock" size={16} /></span>
							{/if}
						</button>
					{/each}
				</div>
			</div>

			<div class="form-actions">
				<!-- 保存後などで空に戻したフォームでは出さず、名前かスケジュールを入れ始めてから出す -->
				{#if pickedSections.length === 0 && (fName.trim() || fCron.trim())}
					<span class="sections-hint">報告する内容を 1 つ以上選んでください。</span>
				{/if}
				{#if editingId != null}
					<Button variant="secondary" onclick={resetForm}>キャンセル</Button>
				{/if}
				<Button variant="primary" type="submit" disabled={saveDisabled}>保存</Button>
			</div>
		</form>
	</div>

	<!-- 登録済み -->
	<div class="card" style="margin-top:24px;">
		<div class="column-header">
			<h3>
				<span class="material-symbols-outlined header-icon-symbol">summarize</span>登録済みの定期報告
			</h3>
		</div>
		<div class="report-list">
			{#if reports.length > 0}
				{#each reports as r (r.id)}
					<div class="card-item glass report-item" class:editing={editingId === r.id}>
						<div class="report-item-body">
							<span class="report-name">{r.name}</span>
							<div class="report-meta">
								<span class="report-cron">{r.schedule_cron}</span>
								<span>{describeTarget(r)}</span>
								<span>{r.sections.map((s) => SECTION_SHORT[s]).join("・")}</span>
							</div>
						</div>
						<div class="entry-actions">
							<button
								type="button"
								class="icon-plain"
								aria-label="「{r.name}」を編集"
								title="編集"
								onclick={() => editReport(r)}
							>
								<Icon name="edit" />
							</button>
							<button
								type="button"
								class="btn-mini"
								class:is-on={r.enabled}
								aria-pressed={r.enabled}
								title={r.enabled ? "クリックで無効にする" : "クリックで有効にする"}
								onclick={() => toggleEnabled(r)}
							>
								{r.enabled ? "有効" : "無効"}
							</button>
							<button
								type="button"
								class="icon-plain danger"
								aria-label="「{r.name}」を削除"
								title="削除"
								onclick={() => deleteReport(r)}
							>
								<Icon name="delete" />
							</button>
						</div>
					</div>
				{/each}
			{:else}
				<EmptyState
					icon="summarize"
					message="定期報告はまだありません。上のフォームから登録できます。"
				/>
			{/if}
		</div>
	</div>

	<!-- テスト配信 -->
	<div class="card" style="margin-top:24px;">
		<div class="column-header">
			<h3>
				<span class="material-symbols-outlined header-icon-symbol">send</span>テスト配信
			</h3>
		</div>
		<div class="test-row">
			{#if testOptions.length > 0}
				<div class="joined-field test-field">
					<Select
						id="delivery-test-kind"
						class="test-select"
						ariaLabel="テスト配信する定期報告"
						options={testOptions}
						bind:value={testId}
					/>
				</div>
			{:else}
				<p class="test-empty">登録済みの定期報告がありません。</p>
			{/if}
			<Button variant="secondary" disabled={testing || !testId} onclick={runTest}>テスト配信</Button>
		</div>
	</div>
</section>

<style>
.report-form {
	display: flex;
	flex-direction: column;
	gap: 12px;
	margin-top: 16px;
	scroll-margin-top: 80px;
}
/* RSS フィード入力: Discord 招待 URL 欄と同じく、入力内に貼り付け・右に追加アイコン。 */
.feed-row {
	display: flex;
	gap: 4px;
}
.feed-input-wrap {
	margin-right: 8px;
}
.feed-input {
	font-family: var(--font-family-mono);
}
/* 押せない追加ボタンは、共通の .icon-action と違い禁止カーソルにする */
.icon-action:disabled {
	cursor: not-allowed;
}
/* スケジュールと報告先の行は、報告先の幅アニメーションのため grid でなく flex で並べる。 */
.schedule-row {
	display: flex;
	flex-wrap: wrap;
	gap: 16px;
}
.schedule-row .form-group {
	margin-bottom: 0;
}
.cron-group {
	flex: 1 1 180px;
}
/* 報告先の枠の幅は固定（中の伸び縮みは TargetField が受け持つ）。 */
.target-group {
	flex: 2 1 240px;
	min-width: 0;
}
.field-hint {
	margin: 6px 0 0;
	font-size: 0.78rem;
	color: var(--text-secondary);
}
/* 天気予報の地域: 地点名と緯度経度の欄を、選んだ側が伸びてもう一方が縮む形で切り替える。 */
.region-pane {
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
.region-pane.active {
	flex-grow: 1;
	opacity: 1;
}
/* 天気設定・ニュースRSS・そのほかの内容: 見出しは .form-group label と同じ見た目。 */
.content-block {
	display: flex;
	flex-direction: column;
	gap: 8px;
	margin: 4px 0 0;
	padding-top: 12px;
	border-top: 1px solid var(--border-divider);
}
.block-title {
	font-size: 0.72rem;
	color: var(--text-secondary);
	font-weight: 700;
	text-transform: uppercase;
	letter-spacing: 0.05em;
}
/* .form-group label の見出し用スタイルをチェックボックスには効かせない */
.content-block :global(.checkbox-label) {
	align-self: flex-start;
	margin: 0;
	color: var(--text-primary);
	font-weight: 400;
	text-transform: none;
	letter-spacing: normal;
}
.block-body {
	display: flex;
	flex-direction: column;
	gap: 8px;
	transition: opacity 0.2s ease;
}
/* 載せない内容の設定欄は薄くして操作できないようにする（値は保持） */
.block-body.off {
	opacity: 0.4;
}
/* フィード一覧: 入力欄の下に、開閉できる罫線区切りリスト。 */
.feed-empty {
	margin: 0;
	font-size: 0.78rem;
	color: var(--text-secondary);
}
.feed-list summary {
	cursor: pointer;
	font-size: 0.8rem;
	color: var(--text-secondary);
	padding: 4px 0;
	user-select: none;
}
.feed-list ul {
	list-style: none;
	margin: 4px 0 0;
	padding: 0;
}
.feed-list li {
	display: flex;
	align-items: center;
	justify-content: space-between;
	gap: 8px;
	padding: 8px 4px;
	border-bottom: 1px solid var(--border-divider);
}
.feed-list li:last-child {
	border-bottom: 0;
}
.feed-url {
	flex: 1;
	min-width: 0;
	overflow: hidden;
	text-overflow: ellipsis;
	white-space: nowrap;
	font-size: 0.85rem;
	font-family: var(--font-family-mono);
}
/* フィード一覧の削除: 枠・背景なしの赤いゴミ箱アイコン。 */
.feed-delete {
	display: flex;
	align-items: center;
	justify-content: center;
	padding: 4px;
	border: 0;
	background: transparent;
	color: var(--color-error);
	cursor: pointer;
}
/* そのほかの内容: 折り返して横に並べる。🔒 は DM のみの内容。 */
.section-options {
	display: flex;
	flex-wrap: wrap;
	gap: 8px 20px;
}
/* 押すと文字色がアクセントカラーになって選択状態を表す（チェックボックスの代わり） */
.section-option {
	display: inline-flex;
	align-items: center;
	gap: 4px;
	padding: 4px 0;
	border: 0;
	background: none;
	color: var(--text-secondary);
	font: inherit;
	cursor: pointer;
	transition: color 0.15s ease;
}
.section-option:hover {
	color: var(--text-primary);
}
.section-option.active {
	color: var(--color-primary);
	font-weight: 700;
}
.section-option.active .lock {
	color: inherit;
}
.lock {
	display: inline-flex;
	color: var(--text-secondary);
}
/* 説明文中の鍵アイコン: 文字の高さに揃える */
.lock-inline {
	display: inline-flex;
	vertical-align: -2px;
}
.form-actions {
	display: flex;
	flex-wrap: wrap;
	align-items: center;
	justify-content: flex-end;
	gap: 8px;
}
.sections-hint {
	margin-right: auto;
	font-size: 0.78rem;
	color: var(--color-error);
}
/* 押せない保存・テスト配信は薄くする（共通ボタンには無効時の見た目が無いため） */
.delivery-tab :global(.btn:disabled) {
	opacity: 0.45;
	cursor: not-allowed;
}
/* 朝報・日報週報の見出し: 右端に ? */
.briefing-header {
	display: flex;
	align-items: center;
	justify-content: space-between;
}
/* 登録済みの一覧: [名前 + 概要] [編集] [有効/無効] [削除] */
.report-list {
	display: flex;
	flex-direction: column;
	gap: 8px;
	margin-top: 16px;
}
.report-item {
	display: flex;
	align-items: center;
	justify-content: space-between;
	gap: 10px;
	/* 共通 .card-item の左右 -12px をやめ、セクションの罫線より横に出ないようにする */
	margin: 0;
}
/* 一番下の項目はセクション（カード）の枠線と重なるので、下罫線を消す */
.report-item:last-child {
	border-bottom: none;
}
.report-item.editing {
	outline: 1px solid var(--color-primary);
}
.report-item-body {
	flex: 1;
	min-width: 0;
}
.report-name {
	font-weight: 600;
}
.report-meta {
	display: flex;
	flex-wrap: wrap;
	gap: 4px 12px;
	margin-top: 4px;
	font-size: 0.78rem;
	color: var(--color-zinc-muted);
}
.report-cron {
	font-family: var(--font-family-mono);
}
/* テスト配信: [定期報告] + ボタン */
.test-row {
	display: flex;
	align-items: stretch;
	gap: 12px;
	margin-top: 16px;
}
.test-field {
	flex: 1;
	min-width: 0;
}
.test-field :global(.test-select) {
	flex: 1;
	min-width: 0;
}
.test-empty {
	flex: 1;
	margin: 0;
	align-self: center;
	font-size: 0.85rem;
	color: var(--text-secondary);
}
.test-row :global(button) {
	white-space: nowrap;
}
</style>
