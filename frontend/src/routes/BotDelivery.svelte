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
let briefingTesting = $state(false);

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
let reportTesting = $state<Record<ReportKind, boolean>>({
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

async function testBriefing() {
	briefingTesting = true;
	try {
		const res = await deliveryApi.testBriefing();
		pushToast(res.message ?? "テスト配信しました。", "success");
	} catch (e) {
		reportError(e);
	} finally {
		briefingTesting = false;
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

async function testReport(kind: ReportKind) {
	reportTesting[kind] = true;
	try {
		const res = await deliveryApi.testReport({ type: kind });
		pushToast(res.message ?? "テスト配信しました。", "success");
	} catch (e) {
		reportError(e);
	} finally {
		reportTesting[kind] = false;
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

<section class="tab-view">
	<!-- 朝報 -->
	<div class="card">
		<div class="column-header action-right">
			<h3>
				<span class="material-symbols-outlined header-icon-symbol">wb_sunny</span>朝報
				(モーニングブリーフィング)
			</h3>
			<Button variant="secondary" small disabled={briefingTesting} onclick={testBriefing}>
				テスト配信
			</Button>
		</div>
		<p class="description-text">
			天気・今日の予定・ニュースをまとめた朝報を毎朝Discordへ配信します。
		</p>
		<form
			style="display:flex;flex-direction:column;gap:12px;margin-top:16px;"
			onsubmit={saveBriefing}
		>
			<div class="form-group" style="display:flex;align-items:center;gap:10px;">
				<Checkbox bind:checked={bEnabled} label="朝報配信を有効にする" />
			</div>
			<div class="form-row">
				<div class="form-group">
					<label for="briefing-cron">配信スケジュール (cron式)</label>
					<input
						id="briefing-cron"
						type="text"
						placeholder="0 7 * * * (毎朝7時)"
						style="font-family:var(--font-family-mono);"
						bind:value={bCron}
					/>
				</div>
				<div class="form-group">
					<label for="briefing-target-type">配信先</label>
					<select id="briefing-target-type" bind:value={bTargetType}>
						<option value="dm">DM</option>
						<option value="channel">チャンネル</option>
					</select>
				</div>
				<div class="form-group">
					<label for="briefing-target-id">チャンネルID (チャンネル選択時)</label>
					<input
						id="briefing-target-id"
						type="text"
						placeholder="例: 123456789012345678"
						bind:value={bTargetId}
					/>
				</div>
			</div>
			<div class="form-row">
				<div class="form-group">
					<label for="briefing-location-name">地点名</label>
					<input id="briefing-location-name" type="text" placeholder="例: 東京" bind:value={bLocation} />
				</div>
				<div class="form-group">
					<label for="briefing-weather-lat">緯度</label>
					<input id="briefing-weather-lat" type="number" step="any" placeholder="35.68" bind:value={bLat} />
				</div>
				<div class="form-group">
					<label for="briefing-weather-lng">経度</label>
					<input id="briefing-weather-lng" type="number" step="any" placeholder="139.76" bind:value={bLng} />
				</div>
			</div>
			<div class="form-group">
				<label for="briefing-feed-input">ニュースRSSフィード</label>
				<div style="display:flex;flex-direction:column;gap:6px;margin-bottom:8px;">
					{#if feeds.length === 0}
						<p style="font-size:0.78rem;color:var(--text-secondary);margin:0;">
							フィードは登録されていません。
						</p>
					{:else}
						{#each feeds as feed, idx (feed)}
							<div
								style="display:flex;align-items:center;justify-content:space-between;gap:8px;padding:6px 10px;border:1px solid var(--border-matte);border-radius:var(--radius);"
							>
								<span
									title={feed}
									style="font-size:0.8rem;font-family:var(--font-family-mono);overflow:hidden;text-overflow:ellipsis;white-space:nowrap;flex:1;"
								>{feed}</span>
								<Button
									variant="trash"
									aria-label="フィードを削除"
									onclick={() => removeFeed(idx)}
								>
									<Icon name="close" size={15} />
								</Button>
							</div>
						{/each}
					{/if}
				</div>
				<div style="display:flex;gap:8px;">
					<input
						id="briefing-feed-input"
						type="url"
						placeholder="https://example.com/rss.xml"
						style="flex-grow:1;"
						bind:value={feedInput}
					/>
					<Button variant="secondary" onclick={addFeed}>＋ フィード追加</Button>
				</div>
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
			<div style="align-self:flex-start;">
				<Button variant="primary" type="submit" disabled={briefingSaving}>
					朝報設定を保存
				</Button>
			</div>
		</form>
	</div>

	<!-- 日報・週報 -->
	<div class="card" style="margin-top:24px;">
		<div class="column-header">
			<h3>
				<span class="material-symbols-outlined header-icon-symbol">summarize</span>日報・週報
			</h3>
		</div>
		<p class="description-text">タスク消化・支出などの活動サマリーを定期配信します。</p>

		<div style="display:flex;flex-direction:column;gap:20px;margin-top:16px;">
			{#each REPORT_META as r (r.kind)}
				<form
					class="report-config-row"
					style="display:flex;flex-wrap:wrap;gap:12px;align-items:flex-end;{r.kind === 'daily' ? 'border-bottom:1px solid var(--border-divider);padding-bottom:16px;' : ''}"
					onsubmit={(e) => saveReport(r.kind, e)}
				>
					<div style="display:flex;align-items:center;gap:10px;min-width:140px;">
						<Checkbox bind:checked={reports[r.kind].enabled} label={r.label} />
					</div>
					<div class="form-group" style="margin:0;flex-grow:1;">
						<label for="report-{r.kind}-cron">配信スケジュール (cron式)</label>
						<input
							id="report-{r.kind}-cron"
							type="text"
							placeholder={r.cronHint}
							style="font-family:var(--font-family-mono);"
							bind:value={reports[r.kind].cron}
						/>
					</div>
					<Button variant="primary" type="submit" disabled={reportSaving[r.kind]}>保存</Button>
					<Button
						variant="secondary"
						disabled={reportTesting[r.kind]}
						onclick={() => testReport(r.kind)}
					>
						テスト配信
					</Button>
				</form>
			{/each}
		</div>
	</div>
</section>
