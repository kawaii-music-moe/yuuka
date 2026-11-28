<script lang="ts">
import { onMount } from "svelte";
import { authApi } from "$lib/api/services";
import { withBasePath } from "$lib/router";

// ログイン先の表示名はサーバーの設定（`OIDC_LABEL`）由来なので、受け取るまでは運用者固有の名前を出さず
// 中立な表記にしておく（GET /api/auth/oidc/provider が返す label で置き換える）。
const FALLBACK_LABEL = "OIDC プロバイダー";
let label = $state(FALLBACK_LABEL);
let enabled = $state(true);

onMount(async () => {
	try {
		const res = await authApi.provider();
		enabled = res.enabled;
		if (res.label) label = res.label;
	} catch {
		// 取得できなくてもボタンは出す（押せばサーバーが理由を返す）。
	}
});

// 招待リンク（`?invite=...`）から来たら、ログイン開始にコードを引き継ぐ（REGISTRATION_MODE=invite）。
const invite =
	new URLSearchParams(window.location.search).get("invite")?.trim() ?? "";

function login(): void {
	const start = "/api/auth/oidc/start";
	window.location.assign(
		invite ? `${start}?invite=${encodeURIComponent(invite)}` : start,
	);
}
</script>

<div class="overlay active" id="login-overlay">
	<div class="login-card">
		<div class="login-header">
			<h1>Yuuka</h1>
			<p>{label} でログインします。</p>
			{#if invite}
				<p class="login-note">招待リンクから登録します。初めての方もこのままログインしてください。</p>
			{/if}
		</div>
		<button type="button" class="btn btn-primary login-button" onclick={login} disabled={!enabled}>
			{label}でログイン
		</button>
		{#if !enabled}
			<p class="login-note">ログインが設定されていません。管理者に連絡してください。</p>
		{/if}
		<div class="login-footer">
			<a href={withBasePath("/terms")}>利用規約</a>
			<a href={withBasePath("/privacy")}>プライバシーポリシー</a>
		</div>
	</div>
</div>

<style>
	.login-card { max-width: 420px; margin: 10vh auto; padding: 32px; text-align: center; }
	.login-button { width: 100%; margin-top: 24px; }
	.login-note { margin-top: 12px; }
	.login-footer { display: flex; justify-content: center; gap: 16px; margin-top: 24px; }
</style>
