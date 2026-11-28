<script lang="ts">
// Discord 連携（後から連携・解除）。docs/design/optional_discord.md §5。
// 連携は Discord の認可画面へのページ遷移で行い、戻ってきたら ?discord=... で結果を伝える。
import { onMount } from "svelte";
import { settingsApi } from "$lib/api/services";
import { confirmDialog } from "$lib/components/ui";
import { pushToast, toastError } from "$lib/stores/toast";

let loaded = $state(false);
let linked = $state(false);
let discordId = $state<string | null>(null);
let available = $state(false);

const RESULTS: Record<string, [string, "success" | "error" | "info"]> = {
	linked: ["Discord を連携しました。", "success"],
	conflict: [
		"この Discord アカウントは別のユーザーに連携されています。管理者に問い合わせてください。",
		"error",
	],
	cancelled: ["Discord の連携を取りやめました。", "info"],
	error: ["Discord を連携できませんでした。もう一度お試しください。", "error"],
	unavailable: ["Discord 連携は設定されていません。", "error"],
};

async function load(): Promise<void> {
	try {
		const res = await settingsApi.discordLink();
		linked = res.linked;
		discordId = res.discordId ?? null;
		available = res.available;
		loaded = true;
	} catch (err) {
		toastError(err, "Discord 連携の状態を読み込めませんでした。");
	}
}

onMount(() => {
	const params = new URLSearchParams(window.location.search);
	const result = params.get("discord");
	if (result && RESULTS[result]) {
		const [message, kind] = RESULTS[result];
		pushToast(message, kind);
		params.delete("discord");
		const query = params.toString();
		const url = `${window.location.pathname}${query ? `?${query}` : ""}`;
		window.history.replaceState(window.history.state, "", url);
	}
	load();
});

function startLink(): void {
	window.location.assign("/api/me/discord/link/start");
}

async function unlink(): Promise<void> {
	const ok = await confirmDialog({
		message:
			"Discord の連携を解除しますか？\nDiscord の Bot との会話や DM 通知は使えなくなり、通知は Web の通知一覧に届くようになります。",
		danger: true,
		confirmLabel: "解除",
	});
	if (!ok) return;
	try {
		const res = await settingsApi.unlinkDiscord();
		if (res.message) pushToast(res.message, "success");
		await load();
	} catch (err) {
		toastError(err);
	}
}
</script>

<div class="config-card card account-card">
	<div class="column-header">
		<h3>Discord 連携</h3>
	</div>
	{#if loaded}
		{#if linked}
			<p class="description-text">連携中の Discord ID: <code>{discordId}</code></p>
			<button type="button" class="btn btn-secondary" onclick={unlink}>連携を解除</button>
		{:else}
			<p class="description-text">
				Discord を連携すると、Discord の Bot との会話や DM での通知が使えます（任意）。
			</p>
			{#if available}
				<button type="button" class="btn btn-primary" onclick={startLink}>Discord を連携</button>
			{:else}
				<p class="description-text">このサーバーでは Discord 連携が設定されていません。</p>
			{/if}
		{/if}
	{/if}
</div>
