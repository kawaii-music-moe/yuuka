<script lang="ts">
	// ─────────────────────────────────────────────────────────────────────────
	// Login — ログイン / アカウント作成 / 初期セットアップ の統合オーバーレイ。
	// 旧 index.html #login-overlay + app.js の各 submit ハンドラ（1442-1631付近）を移植。
	//
	// 画面モード（ローカル state。旧: login-tabs + *-tab-content の .active 排他）:
	//   - "login"     … ログイン（Discord ID + パスワード）
	//   - "register"  … アカウント作成 → DM チャレンジ（確認コード）
	//   - "setup"     … 初期セットアップ Step1（最初の管理者登録。needSetup 時のみ）
	//   - "bot-setup" … 初期セットアップ Step2（system_default Bot トークン登録）
	//
	// setup モードは /api/setup/status の needSetup=true で自動表示（タブ非表示）。
	// ─────────────────────────────────────────────────────────────────────────
	import { onMount } from "svelte";
	import { authApi } from "$lib/api/services";
	import { ApiError } from "$lib/api/client";
	import { bootstrapSession } from "$lib/stores/session";
	import { selectBot } from "$lib/stores/activeBot";
	import { navigateTo } from "$lib/router";

	type Mode = "login" | "register" | "setup" | "bot-setup";

	let mode = $state<Mode>("login");
	let errorMsg = $state("");
	// needSetup=true のあいだは login/register タブを隠す（旧 checkSetupStatus）。
	let needSetup = $state(false);

	// ── ログインフォーム ──
	let loginDiscordId = $state("");
	let loginPassword = $state("");

	// ── 登録フォーム ──
	let regDiscordId = $state("");
	let regUsername = $state("");
	let regPassword = $state("");
	let regGeminiKey = $state("");
	let regInviteCode = $state("");
	// DM チャレンジ（確認コード）ステップ。
	let awaitingVerify = $state(false);
	let verifyCode = $state("");
	let pendingRegisterDiscordId = $state("");

	// ── 初期セットアップ（管理者登録） ──
	let setupDiscordId = $state("");
	let setupUsername = $state("");
	let setupPassword = $state("");
	let setupGeminiKey = $state("");

	// ── デフォルト Bot セットアップ ──
	let botSetupToken = $state("");

	function reportError(e: unknown): void {
		errorMsg = e instanceof ApiError ? e.message : "サーバー接続に失敗しました。";
	}

	// 起動時に setup 要否をプローブ（旧 checkSetupStatus）。needSetup なら setup 表示。
	onMount(async () => {
		try {
			const res = await authApi.setupStatus();
			if (res.needSetup) {
				needSetup = true;
				mode = "setup";
			}
		} catch {
			/* 取得失敗時はログイン画面のまま続行 */
		}
	});

	function switchMode(m: Mode): void {
		mode = m;
		errorMsg = "";
	}

	function continueAfterAuthentication(): void {
		const returnTo = new URLSearchParams(window.location.search).get("returnTo");
		if (returnTo && returnTo.startsWith("/") && !returnTo.startsWith("//")) {
			window.location.assign(returnTo);
			return;
		}
		navigateTo("/");
	}

	// ── ログイン（旧 loginForm submit） ──
	async function submitLogin(e: SubmitEvent): Promise<void> {
		e.preventDefault();
		errorMsg = "";
		try {
			await authApi.login({
				discordId: loginDiscordId.trim(),
				password: loginPassword,
			});
			// セッション再取得 → App 側が isAuthed を検知して Bot 選択へ遷移。
			await bootstrapSession();
			continueAfterAuthentication();
		} catch (e) {
			reportError(e);
		}
	}

	// ── アカウント作成（旧 registerForm submit → DM チャレンジ） ──
	async function submitRegister(e: SubmitEvent): Promise<void> {
		e.preventDefault();
		errorMsg = "";
		const discordId = regDiscordId.trim();
		try {
			const res = await authApi.register({
				discordId,
				username: regUsername.trim(),
				password: regPassword,
				inviteCode: regInviteCode.trim(),
				geminiApiKey: regGeminiKey.trim(),
			});
			if (res.pending) {
				// 確認コード入力ステップへ遷移。
				pendingRegisterDiscordId = discordId;
				awaitingVerify = true;
				verifyCode = "";
			} else {
				// 稀: 即時作成された場合はログインへ誘導。
				switchMode("login");
				loginDiscordId = discordId;
			}
		} catch (e) {
			reportError(e);
		}
	}

	// ── 確認コード検証（旧 registerVerifyForm submit） ──
	async function submitVerify(e: SubmitEvent): Promise<void> {
		e.preventDefault();
		errorMsg = "";
		try {
			await authApi.registerVerify({
				discordId: pendingRegisterDiscordId,
				code: verifyCode.trim(),
			});
			const did = pendingRegisterDiscordId;
			resetRegister();
			switchMode("login");
			loginDiscordId = did;
		} catch (e) {
			reportError(e);
		}
	}

	function resetRegister(): void {
		awaitingVerify = false;
		pendingRegisterDiscordId = "";
		verifyCode = "";
		regDiscordId = "";
		regUsername = "";
		regPassword = "";
		regGeminiKey = "";
		regInviteCode = "";
	}

	// ── 初期セットアップ Step1（旧 setupForm submit） ──
	async function submitSetup(e: SubmitEvent): Promise<void> {
		e.preventDefault();
		errorMsg = "";
		try {
			await authApi.setup({
				discordId: setupDiscordId.trim(),
				username: setupUsername.trim(),
				password: setupPassword,
				geminiApiKey: setupGeminiKey.trim(),
			});
			// 管理者登録直後は自動でログイン済み → デフォルト Bot 設定 Step2 へ。
			await bootstrapSession();
			needSetup = false;
			mode = "bot-setup";
		} catch (e) {
			reportError(e);
		}
	}

	// ── 初期セットアップ Step2（旧 botSetupForm submit） ──
	async function submitBotSetup(e: SubmitEvent): Promise<void> {
		e.preventDefault();
		errorMsg = "";
		try {
			await authApi.setDefaultBotToken({ token: botSetupToken.trim() });
			selectBot({
				id: "system_default",
				name: "システムデフォルト",
				avatar: "",
				preset: "secretary",
			});
			navigateTo("/bot/dashboard");
		} catch (e) {
			reportError(e);
		}
	}
</script>

<div class="overlay active" id="login-overlay">
	<div class="landing-container">
		<!-- 左: アプリ紹介 -->
		<div class="landing-info-section">
			<div class="landing-logo-header">
				<div class="landing-logo-icon-container">
					<span class="material-symbols-outlined landing-logo-icon">calculate</span>
				</div>
				<div class="landing-title-container">
					<h2>Yuuka</h2>
					<span class="landing-subtitle">Seminar Accounting</span>
				</div>
			</div>
			<p class="landing-tagline">
				AIアシスタントと連携したスマートなパーソナル管理システム
			</p>
			<div class="landing-features">
				<div class="landing-feature-card">
					<div class="feature-icon-wrapper">
						<span class="material-symbols-outlined feature-icon">robot_2</span>
					</div>
					<div class="feature-text">
						<h3>AI秘書 Bot (Discord連携)</h3>
						<p>
							Discord上のチャットで会話するだけで、AI秘書がタスク・スケジュール・経費の記録を自動で行います。
						</p>
					</div>
				</div>
				<div class="landing-feature-card">
					<div class="feature-icon-wrapper">
						<span class="material-symbols-outlined feature-icon">checklist</span>
					</div>
					<div class="feature-text">
						<h3>スマートタスク & リマインダー</h3>
						<p>優先度付きタスク管理。滞留や期限をAIが検知して適切にリマインドします。</p>
					</div>
				</div>
				<div class="landing-feature-card">
					<div class="feature-icon-wrapper">
						<span class="material-symbols-outlined feature-icon">payments</span>
					</div>
					<div class="feature-text">
						<h3>AI家計簿 & レシートスキャナー</h3>
						<p>レシート画像を送るだけでGeminiが品目を解析して家計簿に記録します。</p>
					</div>
				</div>
			</div>
		</div>

		<!-- 右: フォーム -->
		<div class="login-card">
			<div class="login-header">
				<div class="logo-icon-container">
					<img src="/materials/yuka.webp" alt="早瀬ユウカ" class="login-avatar-img" />
				</div>
				<h1>Yuuka</h1>
				<p>AIアシスタントと連携したパーソナル管理システム</p>
			</div>

			{#if !needSetup && (mode === "login" || mode === "register")}
				<div class="login-tabs">
					<button
						type="button"
						class="login-tab-btn"
						class:active={mode === "login"}
						onclick={() => switchMode("login")}>ログイン</button
					>
					<button
						type="button"
						class="login-tab-btn"
						class:active={mode === "register"}
						onclick={() => switchMode("register")}>アカウント作成</button
					>
				</div>
			{/if}

			{#if mode === "login"}
				<div class="login-tab-pane active">
					<form onsubmit={submitLogin}>
						<div class="form-group">
							<label for="login-discord-id">Discord ユーザーID *</label>
							<input
								type="text"
								id="login-discord-id"
								required
								placeholder="例: 123456789012345678"
								bind:value={loginDiscordId}
							/>
						</div>
						<div class="form-group">
							<label for="login-password">パスワード *</label>
							<input
								type="password"
								id="login-password"
								required
								placeholder="••••••••••••"
								autocomplete="current-password"
								bind:value={loginPassword}
							/>
						</div>
						<button type="submit" class="btn btn-primary btn-block">アクセスを承認する</button>
					</form>
				</div>
			{:else if mode === "register"}
				<div class="login-tab-pane active">
					{#if !awaitingVerify}
						<form onsubmit={submitRegister}>
							<div class="form-group">
								<label for="reg-discord-id">Discord ユーザーID *</label>
								<input
									type="text"
									id="reg-discord-id"
									required
									placeholder="例: 123456789012345678"
									bind:value={regDiscordId}
								/>
								<span class="field-sub">※Discord上で右クリックしてコピーしたID</span>
							</div>
							<div class="form-group">
								<label for="reg-username">ユーザーネーム（表示名） *</label>
								<input
									type="text"
									id="reg-username"
									required
									placeholder="例: ユーザー名"
									bind:value={regUsername}
								/>
							</div>
							<div class="form-group">
								<label for="reg-password">パスワード *</label>
								<input
									type="password"
									id="reg-password"
									required
									placeholder="••••••••••••"
									autocomplete="new-password"
									bind:value={regPassword}
								/>
							</div>
							<div class="form-group">
								<label for="reg-gemini-key">Gemini API Key *</label>
								<input
									type="password"
									id="reg-gemini-key"
									required
									placeholder="AIzaSy..."
									autocomplete="new-password"
									bind:value={regGeminiKey}
								/>
								<span class="field-sub">※各ユーザーが個別に用意する必要があります。</span>
							</div>
							<div class="form-group">
								<label for="reg-invite-code">招待コード *</label>
								<input
									type="text"
									id="reg-invite-code"
									required
									placeholder="管理者から配布されたコード"
									bind:value={regInviteCode}
								/>
							</div>
							<button type="submit" class="btn btn-primary btn-block">確認コードを受け取る</button>
						</form>
					{:else}
						<form onsubmit={submitVerify}>
							<div style="margin-bottom: 12px; font-size: 0.85rem; color: var(--text-secondary);">
								入力した Discord ID 宛に Bot から確認コードをDMしました。届いたコードを入力してください（10分間有効）。
							</div>
							<div class="form-group">
								<label for="reg-verify-code">確認コード *</label>
								<input
									type="text"
									id="reg-verify-code"
									inputmode="numeric"
									autocomplete="one-time-code"
									pattern="[0-9]*"
									maxlength="6"
									required
									placeholder="6桁のコード"
									bind:value={verifyCode}
								/>
							</div>
							<button type="submit" class="btn btn-primary btn-block">登録を完了する</button>
							<button
								type="button"
								class="btn btn-secondary btn-block"
								style="margin-top: 8px;"
								onclick={() => {
									awaitingVerify = false;
									errorMsg = "";
								}}>入力内容を修正する</button
							>
						</form>
					{/if}
				</div>
			{:else if mode === "setup"}
				<div class="login-tab-pane active">
					<div
						style="margin-bottom: 16px; border-bottom: 1px solid var(--border-divider); padding-bottom: 12px; text-align: center;"
					>
						<h2 style="font-size: 1.25rem; font-weight: 600;">初期セットアップ</h2>
						<p class="field-sub" style="margin-top: 4px;">
							最初のユーザーは自動的にシステム管理者になります。
						</p>
					</div>
					<form onsubmit={submitSetup}>
						<div class="form-group">
							<label for="setup-discord-id">管理者 Discord ユーザーID *</label>
							<input
								type="text"
								id="setup-discord-id"
								required
								placeholder="例: 123456789012345678"
								bind:value={setupDiscordId}
							/>
						</div>
						<div class="form-group">
							<label for="setup-username">管理者ユーザーネーム（表示名） *</label>
							<input
								type="text"
								id="setup-username"
								required
								placeholder="例: 管理者ユーザー名"
								bind:value={setupUsername}
							/>
						</div>
						<div class="form-group">
							<label for="setup-password">管理者パスワード *</label>
							<input
								type="password"
								id="setup-password"
								required
								placeholder="••••••••••••"
								autocomplete="new-password"
								bind:value={setupPassword}
							/>
						</div>
						<div class="form-group">
							<label for="setup-gemini-key">Gemini API Key *</label>
							<input
								type="password"
								id="setup-gemini-key"
								required
								placeholder="AIzaSy..."
								autocomplete="new-password"
								bind:value={setupGeminiKey}
							/>
						</div>
						<button type="submit" class="btn btn-primary btn-block" style="margin-top: 20px;"
							>管理者アカウントを登録する</button
						>
					</form>
				</div>
			{:else if mode === "bot-setup"}
				<div class="login-tab-pane active">
					<div
						style="margin-bottom: 16px; border-bottom: 1px solid var(--border-divider); padding-bottom: 12px; text-align: center;"
					>
						<h2 style="font-size: 1.25rem; font-weight: 600;">デフォルトBotセットアップ</h2>
						<p class="field-sub" style="margin-top: 4px;">
							システム共通で動作するデフォルトBotのトークンを登録してください。
						</p>
					</div>
					<form onsubmit={submitBotSetup}>
						<div class="form-group">
							<label for="bot-setup-token">デフォルト Bot トークン *</label>
							<input
								type="password"
								id="bot-setup-token"
								required
								placeholder="Discord Bot Token を入力"
								autocomplete="new-password"
								bind:value={botSetupToken}
							/>
							<span class="field-sub">※Discord Developer Portalから取得したBotのトークン</span>
						</div>
						<button type="submit" class="btn btn-primary btn-block" style="margin-top: 20px;"
							>デフォルトBotをセットアップする</button
						>
					</form>
				</div>
			{/if}

			{#if errorMsg}
				<div class="error-msg" role="alert">{errorMsg}</div>
			{/if}

			<div
				style="margin-top: 24px; text-align: center; border-top: 1px solid var(--border-divider); padding-top: 16px; display: flex; justify-content: center; gap: 16px; flex-wrap: wrap;"
			>
				<a href="/admin/usage" class="login-footer-link">
					<span class="material-symbols-outlined" style="font-size: 18px;">help</span> 使い方ガイドを読む
				</a>
				<a href="/admin/terms" class="login-footer-link">
					<span class="material-symbols-outlined" style="font-size: 18px;">gavel</span> 利用規約
				</a>
				<a href="/admin/privacy" class="login-footer-link">
					<span class="material-symbols-outlined" style="font-size: 18px;">policy</span> プライバシーポリシー
				</a>
			</div>
		</div>
	</div>
</div>

<style>
	.login-tab-btn {
		cursor: pointer;
		font: inherit;
	}
	.login-footer-link {
		color: var(--color-primary);
		font-size: 0.85rem;
		text-decoration: none;
		display: inline-flex;
		align-items: center;
		gap: 6px;
		font-weight: 500;
	}
</style>
