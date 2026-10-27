<script lang="ts">
// ─────────────────────────────────────────────────────────────────────────
// AccountOverlay — あなたの設定（Bot 非依存の、あなた全体に効く設定）。
// 旧 index.html #account-overlay #tab-account + app.js の各 config フォーム
// （profile / gemini / password / delete-account）と fetchAccountSettings を移植。
// 再構成（docs/design/settings-restructure.md）で、旧 Bot基本設定にあった user スコープの
// 「時刻と通知」「バックアップ」と、旧 Bot設定の「接続端末」もここへ移した。
//
// - 表示名の初期値は currentUser ストア（旧: /api/status → data.user.username）。
// - テーマは theme ストア（ダーク/ライトの2ボタン。旧 .theme-option）。端末ごとの保存なのでカードに「この端末」を出す。
// - 保存は settingsApi（scope:'user'）。確認は confirmDialog、通知は pushToast。
// ─────────────────────────────────────────────────────────────────────────

import { onMount } from "svelte";
import { ApiError } from "$lib/api/client";
import { settingsApi } from "$lib/api/services";
import {
	confirmDialog,
	Icon,
	ScopeBadge,
	SecretInput,
	SecretSetMark,
	Select,
} from "$lib/components/ui";
import { ACCOUNT_SECTIONS } from "$lib/managementSections";
import { navigateTo } from "$lib/router";
import { selectBot } from "$lib/stores/activeBot";
import { currentUser } from "$lib/stores/session";
import { setTheme, type Theme, theme } from "$lib/stores/theme";
import { pushToast } from "$lib/stores/toast";
import BackupCard from "../routes/config/BackupCard.svelte";
import UserSettingsCard from "../routes/config/UserSettingsCard.svelte";
import HelpTip from "../routes/delivery/HelpTip.svelte";
import AccountDevices from "./account/AccountDevices.svelte";
import ManagementOverlayShell from "./ManagementOverlayShell.svelte";

// ── Gemini 設定 ──
let geminiApiKey = $state("");
// DB に API キーが登録済みか（ラベル横のチェック）
let hasGeminiKey = $state(false);
let geminiModel = $state("");
let llmProvider = $state("gemini");
let llmModels = $state<string[]>([]);
let llmLoaded = $state(false);
const PROVIDER_OPTIONS = [{ value: "gemini", label: "Google Gemini" }];
const modelOptions = $derived(llmModels.map((m) => ({ value: m, label: m })));
onMount(() => {
	settingsApi
		.getGemini()
		.then((res) => {
			geminiModel = res.model;
			llmProvider = res.provider;
			llmModels = res.models;
			hasGeminiKey = res.hasApiKey ?? false;
			llmLoaded = true;
		})
		.catch(reportError);
});

// ── 表示名 ──
// 初期値は現在のセッションユーザー名（旧 fetchAccountSettings）。
let username = $state("");
let packagePin = $state("");
let packagePinConfirm = $state("");
let packaging = $state(false);
$effect(() => {
	// currentUser 変化に追従（未編集時のみ埋める。空文字なら未初期化とみなす）。
	if (!username && $currentUser?.username) username = $currentUser.username;
});

const THEMES: { value: Theme; label: string; preview: string }[] = [
	{ value: "dark", label: "ダーク", preview: "dark-preview" },
	{ value: "light", label: "ライト", preview: "light-preview" },
];

function reportError(e: unknown): void {
	pushToast(
		e instanceof ApiError ? e.message : "通信エラーが発生しました。",
		"error",
	);
}

// ── Gemini 設定保存（旧 geminiConfigForm submit） ──
async function saveGemini(e: SubmitEvent): Promise<void> {
	e.preventDefault();
	try {
		await settingsApi.updateGemini({
			apiKey: geminiApiKey.trim() || undefined,
			model: geminiModel,
			provider: llmProvider,
		});
		if (geminiApiKey.trim()) hasGeminiKey = true;
		geminiApiKey = "";
		pushToast("Gemini 設定を更新しました。", "success");
	} catch (e) {
		reportError(e);
	}
}

// ── 表示名保存（旧 profileConfigForm submit） ──
async function saveProfile(e: SubmitEvent): Promise<void> {
	e.preventDefault();
	try {
		await settingsApi.updateProfile({ username: username.trim() });
		// セッション上の表示名も即時反映（旧 initAppSession 再取得の代替）。
		currentUser.update((u) => (u ? { ...u, username: username.trim() } : u));
		pushToast("プロフィールを更新しました。", "success");
	} catch (e) {
		reportError(e);
	}
}

// ── アカウント削除（旧 deleteAccountForm submit。confirm → confirmDialog） ──
async function deleteAccount(e: SubmitEvent): Promise<void> {
	e.preventDefault();
	const ok = await confirmDialog({
		message:
			"本当にアカウントを削除しますか？\n所有するBotや秘書業務データを含むすべての関連データが削除され、この操作は取り消せません。",
		danger: true,
		confirmLabel: "削除する",
	});
	if (!ok) return;
	try {
		const res = await settingsApi.deleteAccount();
		pushToast(res.message ?? "アカウントを削除しました。", "success");
		selectBot(null);
		currentUser.set(null);
		navigateTo("/login");
	} catch (e) {
		reportError(e);
	}
}

async function downloadPackage(e: SubmitEvent): Promise<void> {
	e.preventDefault();
	if (!/^\d{6,12}$/.test(packagePin)) {
		pushToast("PINコードは6〜12桁の半角数字で入力してください。", "error");
		return;
	}
	if (packagePin !== packagePinConfirm) {
		pushToast("確認用PINコードが一致しません。", "error");
		return;
	}
	packaging = true;
	try {
		const { blob, filename } = await settingsApi.downloadPackage(packagePin);
		const url = URL.createObjectURL(blob);
		const link = document.createElement("a");
		link.href = url;
		link.download = filename ?? "yuuka-bots.yuuka";
		link.click();
		URL.revokeObjectURL(url);
		packagePin = "";
		packagePinConfirm = "";
		pushToast(
			"暗号化パッケージを作成しました。PINコードとファイルを別々に保管してください。",
			"success",
		);
	} catch (e) {
		reportError(e);
	} finally {
		packaging = false;
	}
}
</script>


<ManagementOverlayShell
	id="account-overlay"
	title="あなたの設定"
	base="/account"
	sections={ACCOUNT_SECTIONS}
>
	{#snippet children(section)}
<section id="tab-account">
				<!-- 表示名とテーマ（表示名はアカウント、テーマは端末ごと） -->
				{#if section === "profile"}
					<div class="config-card card account-card">
						<div class="column-header badge-right">
							<h3>表示名</h3>
							<ScopeBadge scope="user" />
						</div>
						<form onsubmit={saveProfile} style="display: flex; gap: 12px; margin-top: 12px;">
							<input
								type="text"
								id="config-profile-username"
								required
								placeholder="表示名 (例: ユーザー名)"
								style="flex-grow: 1;"
								bind:value={username}
							/>
							<button type="submit" class="btn btn-primary" style="white-space: nowrap;">保存</button>
						</form>
					</div>
					<div class="config-card card account-card">
						<div class="column-header badge-right">
							<h3>外観テーマ</h3>
							<ScopeBadge scope="client" />
						</div>
						<p class="description-text">管理画面の外観テーマです。このブラウザに保存されます。</p>
						<div class="theme-options">
							{#each THEMES as t (t.value)}
								<button
									type="button"
									class="theme-option"
									class:active={$theme === t.value}
									data-theme={t.value}
									onclick={() => setTheme(t.value)}
								>
									<div class="theme-preview {t.preview}">
										<div class="preview-sidebar"></div>
										<div class="preview-content">
											<div class="preview-card"></div>
											<div class="preview-card"></div>
										</div>
									</div>
									<span>{t.label}</span>
								</button>
							{/each}
						</div>
					</div>
				{/if}

				<!-- 時刻と通知（旧 Bot基本設定 > アシスタント設定） -->
				{#if section === "notifications"}
					<UserSettingsCard />
				{/if}

				<!-- AI モデル（個人の API キーとモデル。PWA の「使用モデル」と同じ値） -->
				{#if section === "ai"}
					<div class="config-card card account-card">
						<form onsubmit={saveGemini} class="ai-form">
							<div class="form-group">
								<div class="label-row">
									<label for="account-llm-provider">AIプロバイダ・モデル</label>
									<HelpTip id="account-ai-help" label="この設定が使われるBot">
										秘書属性のBotは、話しかけた人のこの API キーとモデルで応答します（共有されたBotでも、各自のキーが使われます）。
										汎用属性のBotはこの設定を使わず、Bot設定の「LLM機能」にある Bot 専用のキーとモデルで応答します。
										Web アプリ（PWA）の設定にある「使用モデル」も、この設定と同じです。
									</HelpTip>
								</div>
								<div class="joined-field">
									<Select
										id="account-llm-provider"
										options={PROVIDER_OPTIONS}
										bind:value={llmProvider}
									/>
									<span class="divider" aria-hidden="true"></span>
									<Select
										id="gemini-model-select"
										class="model-select"
										ariaLabel="モデル"
										options={modelOptions}
										bind:value={geminiModel}
									/>
								</div>
							</div>
							<div class="form-group">
								<label for="gemini-api-key">Gemini API Key<SecretSetMark set={hasGeminiKey} /></label>
								<SecretInput
									id="gemini-api-key"
									placeholder="APIキーを変更する場合は入力してください (マスク表示中)"
									bind:value={geminiApiKey}
								/>
							</div>
							<button type="submit" disabled={!llmLoaded} class="btn btn-primary">AI 設定を保存</button>
						</form>
					</div>
				{/if}

				<!-- バックアップと移行（旧 Bot基本設定 > バックアップ + アカウント管理 > 移行パッケージ） -->
				{#if section === "data"}
					<BackupCard />
					<div class="config-card card account-card">
						<div class="column-header">
							<h3><Icon name="inventory_2" class="header-icon-symbol" />Bot移行パッケージ</h3>
						</div>
						<form onsubmit={downloadPackage} class="package-form">
							<div class="form-group">
								<label for="package-pin">PINコード（6〜12桁）</label>
								<SecretInput id="package-pin" inputmode="numeric" minlength={6} maxlength={12} required bind:value={packagePin} />
							</div>
							<div class="form-group">
								<label for="package-pin-confirm">PINコード（確認）</label>
								<SecretInput id="package-pin-confirm" inputmode="numeric" minlength={6} maxlength={12} required bind:value={packagePinConfirm} />
							</div>
							<button type="submit" class="btn btn-primary" disabled={packaging}>
								<Icon name="download" class="icon-button-left" />
								{packaging ? "パッケージ作成中…" : "暗号化してダウンロード"}
							</button>
						</form>
					</div>
				{/if}

				<!-- ログイン中の端末（旧 Bot設定 > 接続端末） -->
				{#if section === "devices"}
					<AccountDevices />
				{/if}

				<!-- アカウント削除 -->
				{#if section === "delete"}
					<p class="description-text" style="margin-top:12px;">
						アカウントを削除すると、あなたが所有するBotや秘書業務データ（ToDo・予定・経費など）を含む関連データがすべて削除されます。<strong
							>この操作は取り消せません。</strong
						>
					</p>
					<form onsubmit={deleteAccount} style="display: flex; flex-direction: column; gap: 12px; margin-top: 16px;">
						<button type="submit" class="btn btn-danger" style="align-self: flex-start;">
							<Icon name="delete_forever" class="icon-button-left" /> アカウントを削除する
						</button>
					</form>
				{/if}
</section>
	{/snippet}
</ManagementOverlayShell>

<style>
	.theme-option {
		cursor: pointer;
		font: inherit;
	}
	.account-card {
		margin-top: 16px;
	}
	.ai-form {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}
	/* ラベルの右に ? を並べる。.form-group label の下余白はこの行で取る */
	/* プロバイダは本来の幅、モデルは残りいっぱい */
	.ai-form :global(.model-select) {
		flex: 1;
		min-width: 0;
	}
	.package-form {
		display: flex;
		flex-direction: column;
		gap: 12px;
		max-width: 480px;
		margin-top: 16px;
	}
</style>
