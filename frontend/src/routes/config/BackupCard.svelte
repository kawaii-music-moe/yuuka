<script lang="ts">
// Google Drive バックアップ設定カード（旧 backup-config-form + btn-trigger-backup）。

import { ApiError } from "$lib/api/client";
import { settingsApi } from "$lib/api/services";
import { Button } from "$lib/components/ui";
import { pushToast } from "$lib/stores/toast";
import type { StatusConfig } from "./configTypes";

interface Props {
	config: StatusConfig | null;
	/** 保存後に /api/status を再取得して最終実行時刻等を反映 */
	onsaved?: () => void;
}
let { config, onsaved }: Props = $props();

let enabled = $state(false);
let folderId = $state("");
let intervalHours = $state(24);
let generations = $state(7);
let lastRun = $state<string | null>(null);
let triggering = $state(false);

$effect(() => {
	const c = config;
	if (!c) return;
	enabled = !!c.backupEnabled;
	folderId = c.backupFolderId === "未設定" ? "" : (c.backupFolderId ?? "");
	intervalHours = c.backupIntervalHours ?? 24;
	generations = c.backupGenerations ?? 7;
	lastRun = c.backupLastRunAt ?? null;
});

async function submit(e: SubmitEvent) {
	e.preventDefault();
	const hours = Math.min(Math.max(Number(intervalHours) || 24, 1), 720);
	const gens = Math.max(Number(generations) || 7, 1);
	try {
		const res = await settingsApi.updateBackup({
			enabled,
			folderId: folderId.trim(),
			intervalHours: hours,
			generations: gens,
		});
		if (res.success) {
			pushToast("バックアップ設定を保存しました。", "success");
			onsaved?.();
		} else {
			pushToast(`設定の保存に失敗しました: ${res.message ?? ""}`, "error");
		}
	} catch (err) {
		pushToast(
			err instanceof ApiError ? err.message : "通信エラーが発生しました。",
			"error",
		);
	}
}

async function trigger() {
	if (!enabled) {
		pushToast(
			"手動バックアップを実行するには、先にバックアップを有効にして設定を保存してください。",
			"error",
		);
		return;
	}
	triggering = true;
	try {
		const res = (await settingsApi.triggerBackup()) as {
			success: boolean;
			message?: string;
			url?: string;
		};
		if (res.success) {
			pushToast(
				`バックアップが完了しました！${res.url ? ` URL: ${res.url}` : ""}`,
				"success",
			);
		} else {
			pushToast(`バックアップ失敗: ${res.message ?? ""}`, "error");
		}
	} catch (err) {
		pushToast(
			err instanceof ApiError
				? err.message
				: "バックアップ処理中にエラーが発生しました。",
			"error",
		);
	} finally {
		triggering = false;
	}
}
</script>

<details class="config-card card">
	<summary class="column-header badge-right">
		<h3><span class="material-symbols-outlined header-icon-symbol">cloud_sync</span>Google Drive バックアップ</h3>
		<span class="badge badge-accent">Beta</span>
	</summary>
	<p class="description-text">
		データベースや設定ファイル、プレイブックのデータをGoogle Driveへ安全にバックアップします。
	</p>
	<form onsubmit={submit} class="backup-form">
		<div class="form-group checkbox-inline">
			<input type="checkbox" id="backup-enable" bind:checked={enabled} />
			<label for="backup-enable">自動バックアップを有効にする</label>
		</div>
		<div class="form-group">
			<label for="backup-folder-id">バックアップ先フォルダID または フォルダURL (空の場合はマイドライブ直下に保存)</label>
			<input
				type="text"
				id="backup-folder-id"
				placeholder="フォルダID または https://drive.google.com/drive/folders/..."
				bind:value={folderId}
			/>
		</div>
		<div class="form-row">
			<div class="form-group">
				<label for="backup-interval-hours">実行間隔 (時間 / 1〜720)</label>
				<input type="number" id="backup-interval-hours" min="1" max="720" placeholder="24" bind:value={intervalHours} />
			</div>
			<div class="form-group">
				<label for="backup-generations">保持する世代数</label>
				<input type="number" id="backup-generations" min="1" placeholder="7" bind:value={generations} />
			</div>
		</div>
		<div class="field-sub last-run">最終実行: {lastRun || "—"}</div>
		<div class="backup-actions">
			<Button type="submit" variant="primary" block>設定を保存</Button>
			<Button variant="secondary" block onclick={trigger} disabled={triggering}>
				{triggering ? "バックアップ実行中..." : "今すぐバックアップ実行"}
			</Button>
		</div>
	</form>
</details>

<style>
	.backup-form {
		display: flex;
		flex-direction: column;
		gap: 12px;
		margin-top: 16px;
	}
	.checkbox-inline {
		display: flex;
		align-items: center;
		gap: 10px;
	}
	.checkbox-inline input {
		width: 20px;
		height: 20px;
	}
	.last-run {
		margin-top: 4px;
	}
	.backup-actions {
		display: flex;
		gap: 10px;
		margin-top: 8px;
	}
</style>
