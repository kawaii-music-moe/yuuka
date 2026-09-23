<script lang="ts">
// ─────────────────────────────────────────────────────────────────────────
// デバイスフロー認可ページ（/device）（旧 app.js showDeviceApprovalView /
//   deviceCodeInput / btnDeviceApprove / btnDeviceDone / btnDeviceCancel
//   + index.html #device-overlay を移植）。
//
//   ?code= は URL（router の page ストア）から取得。承認は deviceApi.approve。
//   入力は formatDeviceCode で XXXX-XXXX に自動整形。ログイン中ユーザー名は session ストア。
// ─────────────────────────────────────────────────────────────────────────

import { ApiError } from "$lib/api/client";
import { deviceApi } from "$lib/api/services";
import { goto, page } from "$lib/router";
import { currentUser } from "$lib/stores/session";
import { formatDeviceCode } from "./deviceUtils";

let code = $state("");
let errorMsg = $state("");
let approving = $state(false);
// 認可成功時に入力フォームを隠して成功メッセージを表示する。
let successText = $state<string | null>(null);

const userDisplay = $derived($currentUser?.username ?? "—");

// URL の ?code= を初期プリフィル（旧 sessionStorage 退避は router の URL 真実に集約）。
$effect(() => {
	const c = $page.searchParams.get("code") ?? "";
	if (c) code = formatDeviceCode(c);
});

function onInput() {
	code = formatDeviceCode(code);
}

async function approve() {
	errorMsg = "";
	const userCode = formatDeviceCode(code);
	if (userCode.length !== 9) {
		errorMsg = "コードは XXXX-XXXX 形式で入力してください。";
		return;
	}
	approving = true;
	try {
		const res = await deviceApi.approve(userCode);
		if (res.success) {
			successText = `${res.device_name || "端末"} を許可しました。アプリに戻ってください。`;
		} else {
			errorMsg = res.message ?? "コードが無効か、有効期限が切れています。";
		}
	} catch (err) {
		errorMsg =
			err instanceof ApiError ? err.message : "サーバー接続に失敗しました。";
	} finally {
		approving = false;
	}
}
</script>

<div id="device-overlay" class="overlay active">
	<div class="usage-card">
		<div class="login-header">
			<div class="device-logo-row">
				<div class="logo-icon-container device-logo">
					<span class="material-symbols-outlined device-logo-icon">devices</span>
				</div>
				<h1 class="device-title">Device Login</h1>
			</div>
			<p class="device-lead">デスクトップアプリからのログインを許可します。</p>
		</div>

		{#if successText}
			<div class="card glass device-success">
				<span class="material-symbols-outlined device-success-icon">check_circle</span>
				<p class="device-success-text">{successText}</p>
				<button type="button" class="btn btn-secondary device-done" onclick={() => goto("/")}>
					ホームに戻る
				</button>
			</div>
		{:else}
			<div class="card device-form">
				<p class="description-text device-desc">
					デスクトップアプリからのログインを許可します。表示されているコードがアプリの画面と一致することを確認してください。
				</p>
				<p class="field-sub device-user">
					ログイン中のユーザー: <strong>{userDisplay}</strong>
				</p>
				<div class="form-group">
					<label for="device-code-input">確認コード（XXXX-XXXX）</label>
					<input
						type="text"
						id="device-code-input"
						autocomplete="off"
						spellcheck="false"
						maxlength="9"
						placeholder="WDJB-MJHT"
						class="device-code-field"
						bind:value={code}
						oninput={onInput}
					/>
				</div>
				{#if errorMsg}
					<div class="error-msg device-error">{errorMsg}</div>
				{/if}
				<div class="device-actions">
					<button type="button" class="btn btn-primary device-btn" onclick={approve} disabled={approving}>
						この端末を許可
					</button>
					<button type="button" class="btn btn-secondary device-btn" onclick={() => goto("/")}>
						キャンセル
					</button>
				</div>
				<p class="field-sub device-warn">
					※フィッシング対策: ご自身のデスクトップアプリからログインを開始した場合のみ許可してください。心当たりのない場合は許可しないでください。
				</p>
			</div>
		{/if}
	</div>
</div>

<style>
	.device-logo-row {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 12px;
		margin-bottom: 12px;
	}
	.device-logo {
		width: 48px;
		height: 48px;
		display: flex;
		align-items: center;
		justify-content: center;
		background-color: var(--surface-1dp);
		border: 1px solid var(--border-matte);
		border-radius: 50%;
	}
	.device-logo-icon {
		font-size: 24px;
		color: var(--color-primary);
	}
	.device-title {
		font-size: 1.5rem;
		margin: 0;
		text-transform: uppercase;
		letter-spacing: 0.05em;
	}
	.device-lead {
		margin-bottom: 16px;
	}
	.device-form {
		padding: 24px;
		text-align: left;
	}
	.device-desc {
		margin-bottom: 16px;
	}
	.device-user {
		margin-bottom: 16px;
	}
	.device-code-field {
		font-family: "JetBrains Mono", monospace;
		text-transform: uppercase;
		letter-spacing: 0.15em;
		font-size: 1.15rem;
		text-align: center;
	}
	.device-error {
		text-align: center;
	}
	.device-actions {
		display: flex;
		gap: 12px;
		margin-top: 16px;
	}
	.device-btn {
		flex: 1;
	}
	.device-warn {
		margin-top: 16px;
	}
	.device-success {
		padding: 24px;
		text-align: center;
	}
	.device-success-icon {
		font-size: 40px;
		color: var(--color-green, #10b981);
		margin-bottom: 8px;
	}
	.device-success-text {
		margin: 0 0 16px;
	}
	.device-done {
		min-width: 150px;
	}
</style>
