<script lang="ts">
	// ─────────────────────────────────────────────────────────────────────────
	// AccountOverlay — アカウント管理（Bot 非依存のアカウント共通設定）。
	// 旧 index.html #account-overlay #tab-account + app.js の各 config フォーム
	// （profile / gemini / password / delete-account）と fetchAccountSettings を移植。
	//
	// - 表示名の初期値は currentUser ストア（旧: /api/status → data.user.username）。
	// - テーマは theme ストア（3ボタン。旧 .theme-option）。
	// - 保存は settingsApi（scope:'user'）。確認は confirmDialog、通知は pushToast。
	// ─────────────────────────────────────────────────────────────────────────
	import { settingsApi } from "$lib/api/services";
	import { ApiError } from "$lib/api/client";
	import { pushToast } from "$lib/stores/toast";
	import { confirmDialog, Icon } from "$lib/components/ui";
	import { currentUser } from "$lib/stores/session";
	import { selectBot } from "$lib/stores/activeBot";
	import { theme, setTheme, type Theme } from "$lib/stores/theme";
	import { navigateTo } from "$lib/router";
	import ManagementOverlayShell from "./ManagementOverlayShell.svelte";

	// ── Gemini 設定 ──
	let geminiApiKey = $state("");
	let geminiModel = $state("gemini-3.1-flash-lite");

	// ── 表示名 ──
	// 初期値は現在のセッションユーザー名（旧 fetchAccountSettings）。
	let username = $state("");
	$effect(() => {
		// currentUser 変化に追従（未編集時のみ埋める。空文字なら未初期化とみなす）。
		if (!username && $currentUser?.username) username = $currentUser.username;
	});

	// ── パスワード変更 ──
	let currentPassword = $state("");
	let newPassword = $state("");

	// ── アカウント削除 ──
	let deletePassword = $state("");

	const THEMES: { value: Theme; label: string; preview: string }[] = [
		{ value: "dark", label: "ダーク", preview: "dark-preview" },
		{ value: "light", label: "ライト", preview: "light-preview" },
		{ value: "blue-archive", label: "BA", preview: "ba-preview" },
	];

	function reportError(e: unknown): void {
		pushToast(e instanceof ApiError ? e.message : "通信エラーが発生しました。", "error");
	}

	// ── Gemini 設定保存（旧 geminiConfigForm submit） ──
	async function saveGemini(e: SubmitEvent): Promise<void> {
		e.preventDefault();
		try {
			await settingsApi.updateGemini({
				apiKey: geminiApiKey.trim() || undefined,
				model: geminiModel,
			});
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

	// ── パスワード変更（旧 passwordConfigForm submit） ──
	async function savePassword(e: SubmitEvent): Promise<void> {
		e.preventDefault();
		if (!currentPassword || !newPassword) return;
		try {
			await settingsApi.changePassword({ currentPassword, newPassword });
			pushToast("パスワードを変更しました。", "success");
			currentPassword = "";
			newPassword = "";
		} catch (e) {
			reportError(e);
		}
	}

	// ── アカウント削除（旧 deleteAccountForm submit。confirm → confirmDialog） ──
	async function deleteAccount(e: SubmitEvent): Promise<void> {
		e.preventDefault();
		if (!deletePassword) return;
		const ok = await confirmDialog({
			message:
				"本当にアカウントを削除しますか？\n所有するBotや秘書業務データを含むすべての関連データが削除され、この操作は取り消せません。",
			danger: true,
			confirmLabel: "削除する",
		});
		if (!ok) return;
		try {
			const res = await settingsApi.deleteAccount({ password: deletePassword });
			pushToast(res.message ?? "アカウントを削除しました。", "success");
			selectBot(null);
			currentUser.set(null);
			navigateTo("/login");
		} catch (e) {
			reportError(e);
		}
	}
</script>

<ManagementOverlayShell id="account-overlay" icon="manage_accounts" title="アカウント管理">
	<section id="tab-account">
				<p class="description-text" style="margin-bottom: 16px;">
					表示名・外観テーマ・ログインパスワードなど、特定のBotに依存しないアカウント共通の設定です。
				</p>

				<!-- Gemini AI 個別設定 -->
				<details class="card collapsible-group" name="acc-acc" style="margin-bottom:16px;padding:20px;">
					<summary>Gemini AI 個別設定（秘書モード用）</summary>
					<p class="description-text" style="margin-top:12px;">
						あなた個人（アカウント共通）の Gemini API Key とモデルです。秘書モードで使用します。
					</p>
					<form onsubmit={saveGemini} style="display: flex; flex-direction: column; gap: 12px; margin-top: 16px;">
						<div class="form-group" style="margin-bottom: 12px;">
							<label for="gemini-api-key">Gemini API Key</label>
							<input
								type="password"
								id="gemini-api-key"
								placeholder="APIキーを変更する場合は入力してください (マスク表示中)"
								autocomplete="new-password"
								bind:value={geminiApiKey}
							/>
						</div>
						<div class="form-group" style="margin-bottom: 12px;">
							<label for="gemini-model-select">Gemini モデル</label>
							<select id="gemini-model-select" required bind:value={geminiModel}>
								<option value="gemini-3.1-flash-lite">Gemini 3.1 Flash Lite (推奨)</option>
								<option value="gemini-2.5-flash">Gemini 2.5 Flash</option>
								<option value="gemini-2.5-pro">Gemini 2.5 Pro</option>
								<option value="gemini-1.5-flash">Gemini 1.5 Flash</option>
								<option value="gemini-1.5-pro">Gemini 1.5 Pro</option>
							</select>
						</div>
						<button type="submit" class="btn btn-primary">Gemini 設定を保存</button>
					</form>
				</details>

				<!-- 表示名（プロフィール） -->
				<details class="card collapsible-group" name="acc-acc" style="margin-bottom:16px;padding:20px;">
					<summary>表示名（プロフィール）</summary>
					<p class="description-text" style="margin-top:12px;">ご自身の表示名を設定できます。</p>
					<form onsubmit={saveProfile} style="display: flex; gap: 12px; margin-top: 16px;">
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
				</details>

				<!-- テーマ設定 -->
				<details class="card collapsible-group" name="acc-acc" style="margin-bottom:16px;padding:20px;">
					<summary>テーマ設定</summary>
					<p class="description-text" style="margin-top:12px;">
						管理画面の外観テーマを選択します。設定はブラウザに保存されます。
					</p>
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
				</details>

				<!-- パスワード変更 -->
				<details class="card collapsible-group" name="acc-acc" style="margin-bottom:16px;padding:20px;">
					<summary>パスワード変更</summary>
					<p class="description-text" style="margin-top:12px;">
						管理画面ログイン用パスワードを変更します。変更すると他端末のセッションは無効化されます。
					</p>
					<form onsubmit={savePassword} style="display: flex; flex-direction: column; gap: 12px; margin-top: 16px;">
						<div class="form-row">
							<div class="form-group">
								<label for="config-current-password">現在のパスワード *</label>
								<input
									type="password"
									id="config-current-password"
									required
									placeholder="••••••••••••"
									autocomplete="current-password"
									bind:value={currentPassword}
								/>
							</div>
							<div class="form-group">
								<label for="config-new-password">新しいパスワード *</label>
								<input
									type="password"
									id="config-new-password"
									required
									placeholder="••••••••••••"
									autocomplete="new-password"
									bind:value={newPassword}
								/>
							</div>
						</div>
						<button type="submit" class="btn btn-primary" style="align-self: flex-start;"
							>パスワードを変更</button
						>
					</form>
				</details>

				<!-- アカウント削除 -->
				<details class="card collapsible-group" name="acc-acc" style="margin-bottom:16px;padding:20px;">
					<summary>アカウントの削除</summary>
					<p class="description-text" style="margin-top:12px;">
						アカウントを削除すると、あなたが所有するBotや秘書業務データ（ToDo・予定・経費など）を含む関連データがすべて削除されます。<strong
							>この操作は取り消せません。</strong
						>
					</p>
					<form onsubmit={deleteAccount} style="display: flex; flex-direction: column; gap: 12px; margin-top: 16px;">
						<div class="form-group">
							<label for="config-delete-password">現在のパスワード *</label>
							<input
								type="password"
								id="config-delete-password"
								required
								placeholder="••••••••••••"
								autocomplete="current-password"
								bind:value={deletePassword}
							/>
						</div>
						<button type="submit" class="btn btn-danger" style="align-self: flex-start;">
							<Icon name="delete_forever" class="icon-button-left" /> アカウントを削除する
						</button>
					</form>
				</details>
	</section>
</ManagementOverlayShell>

<style>
	.theme-option {
		cursor: pointer;
		font: inherit;
	}
</style>
