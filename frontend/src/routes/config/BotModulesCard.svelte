<script lang="ts">
// 有効な機能（モジュール選択）カード（旧 loadBotModules + saveBotModules）。
// ユーザー個別設定。アクセス権のある全 Bot（デフォルト含む）で表示。

import { ApiError } from "$lib/api/client";
import { botAttributeApi } from "$lib/api/services";
import {
	Button,
	Checkbox,
	confirmDialog,
	Icon,
	ScopeBadge,
} from "$lib/components/ui";
import { pushToast } from "$lib/stores/toast";
import type { ModuleItem, ModulesResp } from "./configTypes";

interface Props {
	botId: string;
}
let { botId }: Props = $props();

// enabled はローカルで bind するため id→boolean のマップで保持し、表示は元配列順。
let modules = $state<ModuleItem[]>([]);
let enabledMap = $state<Record<string, boolean>>({});
let visible = $state(false);

const onCount = $derived(modules.filter((m) => enabledMap[m.id]).length);

$effect(() => {
	void botId;
	void load();
});

async function load() {
	try {
		const res = (await botAttributeApi.modules(botId)) as ModulesResp;
		if (!res.success) {
			visible = false;
			return;
		}
		modules = res.modules ?? [];
		const map: Record<string, boolean> = {};
		for (const m of modules) map[m.id] = !!m.enabled;
		enabledMap = map;
		visible = true;
	} catch {
		visible = false;
	}
}

async function save(enabled: string[] | "all", toastOnSuccess: boolean) {
	try {
		const res = await botAttributeApi.updateModules({
			botId,
			enabledModules: enabled,
		});
		if (!res.success || toastOnSuccess)
			pushToast(
				res.message ?? "保存しました。",
				res.success ? "success" : "error",
			);
		await load();
	} catch (err) {
		pushToast(
			err instanceof ApiError ? err.message : "通信エラーが発生しました。",
			"error",
		);
		await load();
	}
}

// チェックの切り替えごとに即時保存する（失敗時は load で元に戻る）。
function saveSelected() {
	const enabled = modules.filter((m) => enabledMap[m.id]).map((m) => m.id);
	void save(enabled, false);
}

async function resetToDefault() {
	const ok = await confirmDialog({
		message: "有効な機能を既定に戻しますか？",
		confirmLabel: "既定に戻す",
	});
	if (ok) void save("all", true);
}
</script>

{#if visible}
	<section class="config-card card">
		<div class="column-header badge-right">
			<ScopeBadge scope="user_bot" />
			<span class="badge badge-accent">{onCount} / {modules.length}</span>
			<Button
				variant="icon-sm"
				title="既定に戻す"
				aria-label="既定に戻す"
				onclick={resetToDefault}><Icon name="restart_alt" /></Button
			>
		</div>
		<div class="modules-grid">
			{#each modules as m (m.id)}
				<label class="module-toggle">
					<Checkbox
						bind:checked={enabledMap[m.id]}
						onchange={saveSelected}
						aria-label={m.label}
					/>
					<div class="module-label">{m.label}</div>
					{#if m.description}
						<div class="field-sub module-desc">{m.description}</div>
					{/if}
				</label>
			{/each}
		</div>
	</section>
{/if}

<style>
	.modules-grid {
		display: flex;
		flex-direction: column;
		margin-top: 12px;
	}
	.module-toggle {
		display: grid;
		grid-template-columns: auto 1fr;
		column-gap: 8px;
		align-items: center;
		padding: 10px 0;
		cursor: pointer;
	}
	.module-toggle + .module-toggle {
		border-top: 1px solid var(--border-matte);
	}
	.module-desc {
		grid-column: 2;
	}
	.module-label {
		font-weight: 600;
	}
</style>
