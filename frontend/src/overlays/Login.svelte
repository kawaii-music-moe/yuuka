<script lang="ts">
import { withBasePath } from "$lib/router";

function login(): void {
	window.location.assign("/api/auth/sk/start");
}
</script>

<div class="overlay active" id="login-overlay">
	<div class="login-card">
		<div class="login-header">
			<h1>Yuuka</h1>
			<p>sk-9 Account のパスキーでログインします。</p>
		</div>
		<button type="button" class="btn btn-primary login-button" onclick={login}>
			sk-9 Accountでログイン
		</button>
		<div class="login-footer">
			<a href={withBasePath("/terms")}>利用規約</a>
			<a href={withBasePath("/privacy")}>プライバシーポリシー</a>
		</div>
	</div>
</div>

<style>
	.login-card { max-width: 420px; margin: 10vh auto; padding: 32px; text-align: center; }
	.login-button { width: 100%; margin-top: 24px; }
	.login-footer { display: flex; justify-content: center; gap: 16px; margin-top: 24px; }
</style>
