<script lang="ts">
import { onMount } from "svelte";
import { authApi } from "$lib/api/services";
import { withBasePath } from "$lib/router";

// 標準のログイン先（sk-account）の表示名。サーバーから受け取るまでの仮の値。
let label = $state("sk-9 Account");
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

function login(): void {
	window.location.assign("/api/auth/oidc/start");
}
</script>

<div class="overlay active" id="login-overlay">
	<div class="login-card">
		<div class="login-header">
			<h1>Yuuka</h1>
			<p>{label} でログインします。</p>
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
