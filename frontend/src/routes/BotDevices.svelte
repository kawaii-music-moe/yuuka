<script lang="ts">
// ─────────────────────────────────────────────────────────────────────────
// 接続端末 タブ（旧 app.js buildDeviceCard / fetchDevices / fetchDesktopDownload
//   + index.html #tab-devices を移植）。
//
// デバイス系は user-scoped（deviceApi は scope:'user'、botId を付けない）。activeBot 非依存。
//   デスクトップ配布状況の取得と、認可済み端末の一覧・失効を扱う。
// ─────────────────────────────────────────────────────────────────────────
import { onMount } from "svelte";
import { ApiError } from "$lib/api/client";
import { deviceApi } from "$lib/api/services";
import type { DesktopDevice } from "$lib/api/services/deviceApi";
import { confirmDialog, EmptyState } from "$lib/components/ui";
import { pushToast } from "$lib/stores/toast";
import DeviceCard from "./devices/DeviceCard.svelte";

let devices = $state<DesktopDevice[]>([]);
let loaded = $state(false);
let loadError = $state(false);

// デスクトップ配布状況。
let downloadAvailable = $state(false);
let downloadMeta = $state("確認中…");

onMount(() => {
	void loadDevices();
	void loadDesktop();
});

async function loadDevices() {
	loadError = false;
	try {
		const res = await deviceApi.list();
		devices = res.success ? (res.devices ?? []) : [];
	} catch {
		devices = [];
		loadError = true;
	} finally {
		loaded = true;
	}
}

async function loadDesktop() {
	downloadAvailable = false;
	downloadMeta = "確認中…";
	try {
		const res = await deviceApi.desktopInfo();
		if (res.success && res.available) {
			const mb = ((res.size ?? 0) / (1024 * 1024)).toFixed(1);
			const ver =
				res.version && res.version !== "unknown" ? `v${res.version} ・ ` : "";
			downloadMeta = `${ver}${mb} MB ・ Windows (x64)`;
			downloadAvailable = true;
		} else {
			downloadMeta =
				"現在配布できるビルドがありません（デプロイ後に利用可能になります）。";
		}
	} catch {
		downloadMeta = "配布情報の取得に失敗しました。";
	}
}

function download() {
	if (!downloadAvailable) return;
	window.location.href = "/api/desktop/download";
}

async function revoke(id: number) {
	const ok = await confirmDialog({
		message: "この端末のアクセスを失効しますか？",
		danger: true,
		confirmLabel: "失効",
	});
	if (!ok) return;
	try {
		const res = await deviceApi.revoke(id);
		if (!res.success) {
			pushToast(res.message ?? "端末の失効に失敗しました。", "error");
		}
	} catch (err) {
		pushToast(
			err instanceof ApiError ? err.message : "端末の失効に失敗しました。",
			"error",
		);
	}
	await loadDevices();
}
</script>

<section class="tab-view">
	<div class="view-actions-card card">
		<div class="header-title">
			<h2 class="section-title">デスクトップアプリ</h2>
			<p class="field-sub section-desc">
				Yuuka デスクトップ版（Windows・常駐オーバーレイ型チャット）をダウンロードできます。起動後、デバイスログインでこのアカウントに接続してください。
			</p>
		</div>
		<div class="download-row">
			<button type="button" class="btn btn-primary" disabled={!downloadAvailable} onclick={download}>
				<span class="material-symbols-outlined">download</span> Windows版をダウンロード
			</button>
			<span class="field-sub">{downloadMeta}</span>
		</div>
	</div>

	<div class="view-actions-card card">
		<div class="header-title">
			<h2 class="section-title">接続端末</h2>
			<p class="field-sub section-desc">
				デバイスログインで認可したデスクトップアプリ等のクライアント一覧です。心当たりのない端末は失効してください。
			</p>
		</div>
	</div>

	<div class="list-container">
		{#if devices.length > 0}
			{#each devices as d (d.id)}
				<DeviceCard device={d} onrevoke={revoke} />
			{/each}
		{:else if loaded && loadError}
			<div class="glass load-error">端末一覧の取得に失敗しました。</div>
		{:else if loaded}
			<EmptyState
				icon="devices"
				message="接続中の端末はありません。デスクトップアプリからログインすると、ここに表示されます。"
			/>
		{/if}
	</div>
</section>

<style>
	.section-title {
		font-size: 1.05rem;
		margin: 0;
	}
	.section-desc {
		margin-top: 4px;
	}
	.download-row {
		margin-top: 12px;
		display: flex;
		align-items: center;
		gap: 12px;
		flex-wrap: wrap;
	}
	.load-error {
		padding: 16px;
	}
</style>
