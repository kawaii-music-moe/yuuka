<script lang="ts">
// ─────────────────────────────────────────────────────────────────────────
// BotSelection — Bot 一覧・作成・切替（管理ポータル）。
// 旧 index.html #bot-selection-overlay + app.js の fetchBotList / renderBotList /
// selectBot / createBotForm / btn-open-* 配線を移植。
//
// - 一覧取得は botApi.list()（scope:'user'）。統計カードは connected/running で集計。
// - Bot 選択は activeBot ストア（selectBot）へ委譲 → /bot/dashboard へ遷移。
// - 作成は共通 Modal。プリセット表示名は botApi.presets() で上書き。
// - 起動・停止（オーナー本人の Bot のみ。旧 統合管理 › Bot ヘルスから移した）/ Discord 同期 /
//   プロフィール編集 / 削除（デフォルト Bot 以外）。
// - あなたの設定 / 連携サービス / 管理者設定（admin のみ）への遷移は HomeSidebar（共通）。
// ─────────────────────────────────────────────────────────────────────────
import { onMount } from "svelte";
import { ApiError } from "$lib/api/client";
import { botApi } from "$lib/api/services";
import type { BotView, PresetOption } from "$lib/api/types";
import { canControlRuntime, controlRuntime } from "$lib/botRuntime";
import { confirmDialog, Icon, Modal, Select } from "$lib/components/ui";
import { navigateTo } from "$lib/router";
import { selectBot } from "$lib/stores/activeBot";
import { currentUser } from "$lib/stores/session";
import { pushToast } from "$lib/stores/toast";
import HomeSidebar from "./HomeSidebar.svelte";

let bots = $state<BotView[]>([]);
let presets = $state<PresetOption[]>([]);
let loading = $state(false);

// 作成モーダル
let createOpen = $state(false);
let newBotName = $state("");
let newBotPreset = $state("secretary");

function reportError(e: unknown): void {
	pushToast(
		e instanceof ApiError ? e.message : "エラーが発生しました",
		"error",
	);
}

// 一覧（/api/bots）はシステムデフォルト Bot を含まない（案内役で個人データを持たないため・issue #140）。
function isDefaultBot(id: string): boolean {
	return id.startsWith("bot_default_");
}

async function loadBots(): Promise<void> {
	loading = true;
	try {
		const res = await botApi.list();
		bots = res.bots ?? [];
	} catch (e) {
		reportError(e);
		bots = [];
	} finally {
		loading = false;
	}
}

async function loadPresets(): Promise<void> {
	try {
		const res = await botApi.presets();
		presets = res.presets ?? [];
	} catch {
		/* 表示名は既定値で続行 */
	}
}

onMount(() => {
	void loadBots();
	void loadPresets();
});

// ── 統計（旧 renderBotList の home-stats） ──
const total = $derived(bots.length);
const online = $derived(bots.filter((b) => b.connected).length);
const connecting = $derived(
	bots.filter((b) => b.running && !b.connected).length,
);
const stopped = $derived(bots.filter((b) => !b.running).length);

// ── プリセット表示名（作成 select 用ラベル。旧 refreshPresetOptions） ──
function presetLabel(preset: string, fallback: string): string {
	const p = presets.find((x) => x.preset === preset);
	return p?.display_name ?? fallback;
}
const presetOptions = $derived([
	{ value: "secretary", label: presetLabel("secretary", "パーソナル秘書") },
	{ value: "mcp_assistant", label: presetLabel("mcp_assistant", "汎用モード") },
]);

// ── Bot カードの表示メタ（旧 renderBotList のステータス計算） ──
function statusOf(bot: BotView): { dot: string; label: string } {
	if (bot.suspended) return { dot: "dot-offline", label: "停止（管理者）" };
	if (bot.connected)
		return {
			dot: "dot-online",
			label: bot.shared ? "共有Botで稼働" : "稼働中",
		};
	if (bot.running) return { dot: "dot-connecting", label: "接続中…" };
	return { dot: "dot-offline", label: "停止中" };
}

function displayName(bot: BotView): string {
	return bot.name;
}

// ── Bot 選択（旧 selectBot） ──
function choose(bot: BotView): void {
	selectBot({
		id: bot.id,
		name: displayName(bot),
		avatar: bot.discord_avatar_url || "",
		preset: bot.preset || "secretary",
	});
	// 2階層化後の入口は一般情報（設定系はサイドバー「Bot設定」ハブ配下へ移動）。
	navigateTo("/bot/dashboard");
}

// ── 起動・停止（旧 統合管理 › Bot ヘルス） ──
async function toggleRuntime(bot: BotView): Promise<void> {
	try {
		const wait = await controlRuntime(bot.running ? "stop" : "start", bot.id);
		setTimeout(() => void loadBots(), wait);
	} catch (e) {
		reportError(e);
	}
}

// ── Discord 同期（旧 syncBtn） ──
async function syncDiscord(bot: BotView): Promise<void> {
	try {
		await botApi.syncDiscord(bot.id);
		await loadBots();
	} catch (e) {
		reportError(e);
	}
}

// ── 削除（旧 delBtn。confirm → confirmDialog） ──
async function removeBot(bot: BotView): Promise<void> {
	const ok = await confirmDialog({
		message: `本当にBot「${bot.name}」を削除しますか？\n紐づく経費やタスクデータも全て削除されます。`,
		danger: true,
		confirmLabel: "削除",
	});
	if (!ok) return;
	try {
		await botApi.remove(bot.id);
		await loadBots();
	} catch (e) {
		reportError(e);
	}
}

// ── 作成（旧 createBotForm submit） ──
function openCreate(): void {
	newBotName = "";
	newBotPreset = "secretary";
	createOpen = true;
}

async function submitCreate(e: SubmitEvent): Promise<void> {
	e.preventDefault();
	try {
		const res = await botApi.create({
			name: newBotName.trim(),
			preset: newBotPreset,
		});
		createOpen = false;
		if (newBotPreset === "mcp_assistant" && res.message) {
			pushToast(res.message, "info", 8000);
		}
		await loadBots();
	} catch (e) {
		reportError(e);
	}
}

// スマホ用ドロワー開閉（BotShell と同規約。PC はサイドバー常設）。
let sidebarOpen = $state(false);
</script>

<div class="app-container">
	<HomeSidebar bind:open={sidebarOpen} />

	<main class="main-content">
		<!-- スマホ専用バー（☰ + ブランド） -->
		<header class="home-mobile-bar">
			<button
				type="button"
				class="menu-toggle"
				aria-label="メニューを開く"
				onclick={() => (sidebarOpen = true)}
			>
				<Icon name="menu" />
			</button>
			<span class="home-nav-title">Yuuka</span>
		</header>

		<div class="home-dashboard">

		<!-- グリーティング + サマリー -->
		<div class="home-hero">
			<div class="home-greeting">
				<h1 class="home-greeting-title">
					おかえりなさい、{$currentUser?.username ?? "—"}
				</h1>
			</div>
			<div class="home-stats">
				<div class="home-stat-card">
					<div class="home-stat-label">TOTAL BOTS</div>
					<div class="home-stat-value">{total}</div>
				</div>
				<div class="home-stat-card stat-online">
					<div class="home-stat-label">ONLINE</div>
					<div class="home-stat-value">{online}</div>
				</div>
				<div class="home-stat-card" class:stat-warning={connecting > 0}>
					<div class="home-stat-label">CONNECTING</div>
					<div class="home-stat-value">{connecting}</div>
				</div>
				<div class="home-stat-card">
					<div class="home-stat-label">STOPPED</div>
					<div class="home-stat-value">{stopped}</div>
				</div>
			</div>
		</div>

		<!-- Bot セクション -->
		<div class="home-section">
			<div class="home-section-header">
				<div class="home-section-label">
					<span class="material-symbols-outlined home-section-icon">robot_2</span>
					<h2 class="home-section-title">アシスタント Bot</h2>
				</div>
				<button type="button" class="btn btn-primary btn-sm" onclick={openCreate}>
					<Icon name="add" class="icon-button-left" />新規作成
				</button>
			</div>

			<div class="bot-cards-grid">
				{#each bots as bot (bot.id)}
					{@const st = statusOf(bot)}
					<div
						class="bot-item-card"
						class:default-bot={isDefaultBot(bot.id)}
						role="button"
						tabindex="0"
						onclick={() => choose(bot)}
						onkeydown={(e) => {
							if (e.key === "Enter" || e.key === " ") {
								e.preventDefault();
								choose(bot);
							}
						}}
					>
						<div class="bot-card-top">
							<div class="bot-avatar">
								{#if bot.discord_avatar_url}
									<img src={bot.discord_avatar_url} alt={displayName(bot)} />
								{:else}
									<span class="material-symbols-outlined" style="font-size: 22px;">robot_2</span>
								{/if}
							</div>
							<div class="bot-info">
								<div class="bot-name" title={displayName(bot)}>{displayName(bot)}</div>
								<div class="bot-status">
									{bot.preset_display_name ||
										(bot.preset === "mcp_assistant" ? "汎用モード" : "パーソナル秘書")}
								</div>
							</div>
						</div>
						<div class="bot-card-footer">
							<div class="bot-run-status">
								<span class="bot-run-dot {st.dot}"></span>{st.label}
							</div>
							<div class="bot-item-actions">
								{#if canControlRuntime(bot, $currentUser?.discordId)}
									<button
										type="button"
										class="btn-sync-discord"
										title={bot.running ? "Botを停止" : "Botを起動"}
										aria-label={bot.running ? "Botを停止" : "Botを起動"}
										onclick={(e) => {
											e.stopPropagation();
											void toggleRuntime(bot);
										}}
									>
										<Icon name={bot.running ? "stop_circle" : "play_circle"} size={15} />
									</button>
								{/if}
								<button
									type="button"
									class="btn-sync-discord"
									title="Discordから名前・アイコンを同期"
									onclick={(e) => {
										e.stopPropagation();
										void syncDiscord(bot);
									}}
								>
									<Icon name="sync" size={15} />
								</button>
								{#if !isDefaultBot(bot.id)}
									<button
										type="button"
										class="btn-delete-bot-inline"
										title="Botを削除"
										onclick={(e) => {
											e.stopPropagation();
											void removeBot(bot);
										}}
									>
										<Icon name="delete" size={15} />
									</button>
								{/if}
							</div>
						</div>
					</div>
				{/each}
			</div>
		</div>
		</div>
	</main>
</div>

<style>
	/* BotShell と同じ骨格（scoped のため各ページで宣言が必要） */
	.app-container {
		display: flex;
		min-height: 100vh;
	}
	/* スマホ専用バー（PC ではサイドバー常設のため非表示） */
	.home-mobile-bar {
		display: none;
	}
	@media (max-width: 768px) {
		.home-mobile-bar {
			display: flex;
			align-items: center;
			gap: 10px;
			margin-bottom: 14px;
		}
	}
</style>

<!-- Bot 作成モーダル -->
<Modal bind:open={createOpen} title="新規Botの作成">
	<form onsubmit={submitCreate}>
		<div class="form-group">
			<label for="new-bot-name">Botの名前 *</label>
			<input
				type="text"
				id="new-bot-name"
				required
				placeholder="例: アシスタント"
				bind:value={newBotName}
			/>
			<span class="field-sub">※AIのキャラクター設定は作成後に「ペルソナ」タブで管理できます。</span>
		</div>
		<div class="form-group">
			<label for="new-bot-preset">Botの種類（プリセット）</label>
			<Select id="new-bot-preset" options={presetOptions} bind:value={newBotPreset} />
			<span class="field-sub"
				>※パーソナル秘書: 現行のフル機能（個人向け）。汎用モード: MCP接続・ペルソナ・メモリのみのサーバー常駐Bot（Bot専用のGemini
				APIキーが必要）。</span
			>
		</div>
		<button type="submit" class="btn btn-primary btn-block">作成する</button>
	</form>
</Modal>
