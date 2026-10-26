<script lang="ts">
// ─────────────────────────────────────────────────────────────────────────
// 配信設定タブ（旧 app.js fetchBriefingConfig / fetchReportConfigs +
//  index.html #tab-delivery を移植）。deliveryApi 使用（scope:'bot'）。
//   - 朝報（モーニングブリーフィング）: 有効/cron/配信先/天気地点/RSSフィード/キーワード
//   - 日報・週報: 有効/cron の定期配信 + テスト配信
// bot-scoped のため activeBot 切替に $effect で追従する。
// ─────────────────────────────────────────────────────────────────────────

import { ApiError } from "$lib/api/client";
import { deliveryApi } from "$lib/api/services";
import { Button, Checkbox, Icon } from "$lib/components/ui";
import { activeBot } from "$lib/stores/activeBot";
import { pushToast } from "$lib/stores/toast";
import CronField from "./delivery/CronField.svelte";
import HelpTip from "./delivery/HelpTip.svelte";
import InlineSelect from "./delivery/InlineSelect.svelte";

// ── 朝報フォーム state ──
let bEnabled = $state(false);
let bCron = $state("");
let bTargetType = $state<"dm" | "channel">("dm");
let bTargetId = $state("");
let bLocation = $state("");
let bLat = $state("");
let bLng = $state("");
let bKeywords = $state("");
let feeds = $state<string[]>([]);
let feedInput = $state("");
let briefingSaving = $state(false);
// 配信先プルダウンの選択肢
const TARGET_OPTIONS: { value: "dm" | "channel"; label: string }[] = [
	{ value: "dm", label: "DM" },
	{ value: "channel", label: "チャンネル" },
];
// 天気予報の地域は地点名か緯度経度のどちらで指定するか（表示の切替のみ。保存は両方送る）
type RegionMode = "name" | "coords";
const REGION_OPTIONS: { value: RegionMode; label: string }[] = [
	{ value: "name", label: "地点名" },
	{ value: "coords", label: "緯度経度" },
];
let regionMode = $state<RegionMode>("name");

// ── 日報/週報フォーム state ──
type ReportKind = "daily" | "weekly";
let reports = $state<Record<ReportKind, { enabled: boolean; cron: string }>>({
	daily: { enabled: false, cron: "" },
	weekly: { enabled: false, cron: "" },
});
let reportSaving = $state<Record<ReportKind, boolean>>({
	daily: false,
	weekly: false,
});

function reportError(e: unknown) {
	pushToast(
		e instanceof ApiError ? e.message : "エラーが発生しました",
		"error",
	);
}

async function loadBriefing() {
	try {
		const res = await deliveryApi.getBriefingConfig();
		const c = res.config;
		bEnabled = !!c?.enabled;
		bCron = c?.schedule_cron ?? "";
		bTargetType = c?.target_type === "channel" ? "channel" : "dm";
		bTargetId = c?.target_id ?? "";
		bLocation = c?.location_name ?? "";
		bLat = c?.weather_lat != null ? String(c.weather_lat) : "";
		bLng = c?.weather_lng != null ? String(c.weather_lng) : "";
		regionMode = bLat !== "" || bLng !== "" ? "coords" : "name";
		bKeywords = Array.isArray(c?.news_keywords)
			? c.news_keywords.join(", ")
			: "";
		feeds = Array.isArray(c?.news_feeds) ? c.news_feeds.slice() : [];
	} catch (e) {
		reportError(e);
	}
}

async function loadReports() {
	try {
		const res = await deliveryApi.reportConfigs();
		for (const c of res.configs ?? []) {
			if (c.type === "daily" || c.type === "weekly") {
				reports[c.type] = {
					enabled: !!c.enabled,
					cron: c.schedule_cron ?? "",
				};
			}
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

async function saveBriefing(e: SubmitEvent) {
	e.preventDefault();
	briefingSaving = true;
	try {
		const keywords = bKeywords
			.split(",")
			.map((k) => k.trim())
			.filter((k) => k.length > 0);
		const payload = {
			enabled: bEnabled,
			...(bCron.trim() ? { schedule_cron: bCron.trim() } : {}),
			target_type: bTargetType,
			target_id: bTargetId.trim(),
			weather_lat: bLat === "" ? null : Number(bLat),
			weather_lng: bLng === "" ? null : Number(bLng),
			location_name: bLocation.trim(),
			news_feeds: feeds,
			news_keywords: keywords,
		};
		const res = await deliveryApi.saveBriefingConfig(payload);
		pushToast(res.message ?? "保存しました。", "success");
	} catch (e) {
		reportError(e);
	} finally {
		briefingSaving = false;
	}
}

async function saveReport(kind: ReportKind, e: SubmitEvent) {
	e.preventDefault();
	reportSaving[kind] = true;
	try {
		const cron = reports[kind].cron.trim();
		const payload: Record<string, unknown> = {
			type: kind,
			enabled: reports[kind].enabled,
			...(cron ? { schedule_cron: cron } : {}),
		};
		const res = await deliveryApi.saveReportConfig(payload);
		pushToast(res.message ?? "保存しました。", "success");
	} catch (e) {
		reportError(e);
	} finally {
		reportSaving[kind] = false;
	}
}

// ── テスト配信（朝報・日報・週報をまとめて 1 か所から） ──
type TestKind = "briefing" | ReportKind;
const TEST_OPTIONS: { value: TestKind; label: string }[] = [
	{ value: "briefing", label: "朝報" },
	{ value: "daily", label: "日報" },
	{ value: "weekly", label: "週報" },
];
let testKind = $state<TestKind>("briefing");
let testing = $state(false);

async function runTest() {
	testing = true;
	try {
		const res =
			testKind === "briefing"
				? await deliveryApi.testBriefing()
				: await deliveryApi.testReport({ type: testKind });
		pushToast(res.message ?? "テスト配信しました。", "success");
	} catch (e) {
		reportError(e);
	} finally {
		testing = false;
	}
}

const REPORT_META: { kind: ReportKind; label: string; cronHint: string }[] = [
	{ kind: "daily", label: "日報", cronHint: "0 22 * * * (毎晩22時)" },
	{ kind: "weekly", label: "週報", cronHint: "0 21 * * 0 (毎週日曜21時)" },
];

// activeBot 切替に追従して両設定を再取得
$effect(() => {
	void $activeBot?.id;
	loadBriefing();
	loadReports();
});
</script>

<section class="tab-view delivery-tab">
	<!-- 朝報 -->
	<div class="card">
		<div class="column-header briefing-header">
			<h3>
				<span class="material-symbols-outlined header-icon-symbol">wb_sunny</span>朝報
				(モーニングブリーフィング)
			</h3>
			<HelpTip
				id="briefing-help"
				label="朝報で配信される内容"
				align="end"
				text="毎朝、天気予報・今日の予定・ニュース（登録した RSS フィードから、キーワードで絞り込み）をまとめて Discord へ配信します。"
			/>
		</div>
		<form
			style="display:flex;flex-direction:column;gap:12px;margin-top:16px;"
			onsubmit={saveBriefing}
		>
			<div class="form-group" style="display:flex;align-items:center;gap:10px;">
				<Checkbox bind:checked={bEnabled} label="朝報配信を有効にする" />
			</div>
			<div class="schedule-row">
				<div class="form-group cron-group">
					<CronField id="briefing-cron" placeholder="0 7 * * * (毎朝7時)" bind:value={bCron} />
				</div>
				<div class="form-group target-group" class:open={bTargetType === "channel"}>
					<label for="briefing-target-type">配信先</label>
					<div class="joined-field">
						<InlineSelect
							id="briefing-target-type"
							class="target-select"
							options={TARGET_OPTIONS}
							bind:value={bTargetType}
						/>
						<div class="target-extra" inert={bTargetType !== "channel"}>
								<span class="divider" aria-hidden="true"></span>
								<input
									id="briefing-target-id"
									type="text"
									autocomplete="off"
									placeholder="チャンネルID（例: 123456789012345678）"
									aria-label="チャンネルID"
									bind:value={bTargetId}
								/>
						</div>
					</div>
				</div>
			</div>
			<div class="form-group">
				<label for="briefing-region-mode">天気予報の地域</label>
				<div class="joined-field">
					<InlineSelect id="briefing-region-mode" options={REGION_OPTIONS} bind:value={regionMode} />
					<span class="divider" aria-hidden="true"></span>
					<div class="region-pane" class:active={regionMode === "name"} inert={regionMode !== "name"}>
						<input
							type="text"
							autocomplete="off"
							placeholder="地点名（例: 東京）"
							aria-label="地点名"
							bind:value={bLocation}
						/>
					</div>
					<div class="region-pane" class:active={regionMode === "coords"} inert={regionMode !== "coords"}>
						<input
							type="number"
							step="any"
							placeholder="緯度（例: 35.68）"
							aria-label="緯度"
							bind:value={bLat}
						/>
						<span class="divider" aria-hidden="true"></span>
						<input
							type="number"
							step="any"
							placeholder="経度（例: 139.76）"
							aria-label="経度"
							bind:value={bLng}
						/>
					</div>
				</div>
			</div>
			<div class="form-group">
				<label for="briefing-feed-input">ニュースRSSフィード</label>
				<div class="feed-row">
					<div class="feed-input-wrap">
						<input
							id="briefing-feed-input"
							type="url"
							autocomplete="off"
							placeholder="https://example.com/rss.xml"
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
							class="icon-button paste-button"
							onclick={pasteFeed}
							aria-label="貼り付け"
							title="貼り付け"
						>
							<span class="material-symbols-outlined" aria-hidden="true">content_paste</span>
						</button>
					</div>
					<button
						type="button"
						class="icon-button"
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
			</div>
			<div class="form-group">
				<label for="briefing-keywords">ニュース絞り込みキーワード (カンマ区切り・任意)</label>
				<input
					id="briefing-keywords"
					type="text"
					placeholder="例: AI, 天文, ゲーム"
					bind:value={bKeywords}
				/>
			</div>
			<div style="align-self:flex-end;">
				<Button variant="primary" type="submit" disabled={briefingSaving || !bCron.trim()}>
					保存
				</Button>
			</div>
		</form>
	</div>

	<!-- 日報・週報 -->
	<div class="card" style="margin-top:24px;">
		<div class="column-header briefing-header">
			<h3>
				<span class="material-symbols-outlined header-icon-symbol">summarize</span>日報・週報
			</h3>
			<HelpTip
				id="report-help"
				label="日報・週報で配信される内容"
				align="end"
				text="完了・持ち越しタスク、予定や支払い、家計（収入・支出）、会話のトピックをまとめた活動サマリーを、日報は直近 24 時間分、週報は直近 7 日分で Discord へ配信します。"
			/>
		</div>

		<div class="report-list" style="display:flex;flex-direction:column;gap:20px;margin-top:16px;">
			{#each REPORT_META as r (r.kind)}
				<form
					class="report-config-row"
					style="display:flex;gap:12px;align-items:flex-end;{r.kind === 'daily' ? 'border-bottom:1px solid var(--border-divider);padding-bottom:16px;' : ''}"
					onsubmit={(e) => saveReport(r.kind, e)}
				>
					<div class="form-group report-cron" style="margin:0;flex-grow:1;">
						<div class="report-enable">
							<Checkbox bind:checked={reports[r.kind].enabled} label={r.label} />
						</div>
						<CronField id="report-{r.kind}-cron" placeholder={r.cronHint} bind:value={reports[r.kind].cron} />
					</div>
					<div class="report-actions">
						<span class="save-text">
							<Button
								variant="primary"
								type="submit"
								disabled={reportSaving[r.kind] || !reports[r.kind].cron.trim()}
							>
								保存
							</Button>
						</span>
						<button
							type="submit"
							class="icon-button save-button"
							disabled={reportSaving[r.kind] || !reports[r.kind].cron.trim()}
							aria-label="{r.label}の設定を保存"
							title="保存"
						>
							<span class="material-symbols-outlined" aria-hidden="true">save</span>
						</button>
					</div>
				</form>
			{/each}
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
			<div class="joined-field test-field">
				<InlineSelect
					id="delivery-test-kind"
					class="test-select"
					ariaLabel="テスト配信する内容"
					options={TEST_OPTIONS}
					bind:value={testKind}
				/>
			</div>
			<Button variant="secondary" disabled={testing} onclick={runTest}>テスト配信</Button>
		</div>
	</div>
</section>

<style>
/* 入力欄は Discord 設定ページと同じく枠線・フォーカスリングなし。 */
.delivery-tab input,
.delivery-tab input:focus {
	border: 0;
	outline: none;
	box-shadow: none;
}
/* 緯度・経度の増減スピンボタンを消す（Chrome / Firefox）。 */
.delivery-tab input[type="number"] {
	appearance: textfield;
	-moz-appearance: textfield;
}
.delivery-tab input[type="number"]::-webkit-outer-spin-button,
.delivery-tab input[type="number"]::-webkit-inner-spin-button {
	-webkit-appearance: none;
	margin: 0;
}
/* RSS フィード入力: Discord 招待 URL 欄と同じく、入力内に貼り付け・右に追加アイコン。 */
.feed-row {
	display: flex;
	gap: 4px;
}
.feed-input-wrap {
	position: relative;
	flex: 1;
	min-width: 0;
	margin-right: 8px;
}
.feed-input {
	width: 100%;
	box-sizing: border-box;
	padding-right: 44px;
	font-family: var(--font-family-mono);
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
}
.icon-button:disabled {
	color: var(--text-secondary);
	opacity: 0.4;
	cursor: not-allowed;
}
.paste-button {
	position: absolute;
	right: 4px;
	top: 50%;
	transform: translateY(-50%);
	width: 36px;
	height: 32px;
	color: var(--text-secondary);
}
/* 配信先: 種別プルダウンとチャンネルID を 1 つの枠に並べる（Discord 設定ページと同じ）。 */
/* cron と配信先の行は、配信先の幅アニメーションのため grid でなく flex で並べる。 */
.schedule-row {
	display: flex;
	flex-wrap: wrap;
	gap: 16px;
}
.cron-group {
	flex: 1 1 180px;
}
/* 枠の幅は固定。DM のときはプルダウンが枠いっぱいに広がり、チャンネルにすると
   プルダウンが本来の幅へ縮んで、空いた右側に ID 入力欄が伸びてくる。 */
.target-group {
	flex: 2 1 240px;
	min-width: 0;
}
.joined-field {
	display: flex;
	min-width: 0;
	background-color: var(--surface-1dp);
	border-radius: 4px;
}
.joined-field input {
	flex: 1;
	min-width: 0;
	background: transparent;
}
.target-group :global(.target-select) {
	flex: 1 0 auto;
	transition: flex-grow 0.3s ease;
}
.target-group.open :global(.target-select) {
	flex-grow: 0;
}
.target-extra {
	display: flex;
	flex: 1 1 0;
	width: 0;
	min-width: 0;
	overflow: hidden;
	flex-grow: 0;
	opacity: 0;
	transition:
		flex-grow 0.3s ease,
		opacity 0.2s ease;
}
.target-group.open .target-extra {
	flex-grow: 1;
	opacity: 1;
}
.target-extra input {
	flex: 1;
	min-width: 0;
}
.joined-field .divider {
	width: 1px;
	flex-shrink: 0;
	margin: 8px 0 8px 12px;
	background-color: var(--border-divider);
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
/* フィード一覧: 入力欄の下に、開閉できる罫線区切りリスト。 */
.feed-empty {
	margin: 8px 0 0;
	font-size: 0.78rem;
	color: var(--text-secondary);
}
.feed-list {
	margin-top: 8px;
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
/* 日報・週報の行は折り返さず 1 行に並べ、幅が足りなくなったら保存をアイコンにする。 */
.report-list {
	container-type: inline-size;
}
.report-cron {
	min-width: 0;
}
/* 有効化チェックボックスは入力欄の上に置く */
.report-enable {
	display: flex;
	align-items: center;
	margin-bottom: 12px;
}
.report-actions :global(button) {
	white-space: nowrap;
}
/* 保存（枠なし）とテスト配信（1px 枠あり）の高さ・位置を揃える。 */
/* 朝報・日報週報の見出し: 右端に ? */
.briefing-header {
	display: flex;
	align-items: center;
	justify-content: space-between;
}
/* テスト配信: [朝報/日報/週報] + ボタン */
.test-row {
	display: flex;
	align-items: stretch;
	gap: 12px;
	margin-top: 16px;
}
.test-field {
	flex: 1;
}
.test-field :global(.test-select) {
	flex: 1;
}
.test-row :global(button) {
	white-space: nowrap;
}
/* 保存を、チェックボックス・入力欄と同じ行に並べる。 */
.report-actions {
	display: flex;
	align-items: center;
	gap: 8px;
	/* 入力欄（下端揃え）と高さを合わせる */
	min-height: 46px;
	flex-shrink: 0;
}
/* 保存は PC では文字のボタン、スマホでは枠・背景なしのアイコンだけにする。 */
.save-text {
	display: contents;
}
.save-button {
	display: none;
	align-self: stretch;
}
@container (max-width: 640px) {
	.save-text {
		display: none;
	}
	.save-button {
		display: flex;
	}
}
</style>
