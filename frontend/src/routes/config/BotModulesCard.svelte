<script lang="ts">
// 有効な機能（モジュール選択）カード（旧 loadBotModules + saveBotModules）。
// ユーザー個別設定。アクセス権のある全 Bot（デフォルト含む）で表示。

import { ApiError } from "$lib/api/client";
import { botAttributeApi } from "$lib/api/services";
import { Button, Checkbox } from "$lib/components/ui";
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

async function save(enabled: string[] | "all") {
	try {
		const res = await botAttributeApi.updateModules({
			botId,
			enabledModules: enabled,
		});
		pushToast(
			res.message ?? "保存しました。",
			res.success ? "success" : "error",
		);
		if (res.success) await load();
	} catch (err) {
		pushToast(
			err instanceof ApiError ? err.message : "通信エラーが発生しました。",
			"error",
		);
	}
}

function saveSelected() {
	const enabled = modules.filter((m) => enabledMap[m.id]).map((m) => m.id);
	void save(enabled);
}
</script>

{#if visible}
	<details class="config-card card" open>
		<summary class="column-header badge-right">
			<h3><span class="material-symbols-outlined header-icon-symbol">tune</span>有効な機能（あなた専用）</h3>
			<span class="badge badge-accent">{onCount} / {modules.length}</span>
		</summary>
		<p class="description-text">
			あなたがこのBotで使う機能だけを選べます（この設定はあなた個人にのみ適用されます）。オフにした機能はAIに渡されず、左メニューの該当タブも非表示になります。変更は次のメッセージ処理から反映されます。
		</p>
		<div class="modules-grid">
			{#each modules as m (m.id)}
				<label class="module-toggle">
					<Checkbox bind:checked={enabledMap[m.id]} aria-label={m.label} />
					<div>
						<div class="module-label">{m.label}</div>
						<div class="field-sub">{m.description || ""}</div>
					</div>
				</label>
			{/each}
		</div>
		<div class="modules-actions">
			<Button variant="primary" onclick={saveSelected}>機能を保存</Button>
			<Button variant="secondary" onclick={() => save("all")}>既定に戻す</Button>
		</div>
	</details>
{/if}

<style>
	.modules-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
		gap: 8px;
		margin-top: 12px;
	}
	.module-toggle {
		display: flex;
		gap: 8px;
		align-items: flex-start;
		padding: 8px;
		border: 1px solid var(--border-matte);
		border-radius: 8px;
		cursor: pointer;
	}
	.module-label {
		font-weight: 600;
	}
	.modules-actions {
		display: flex;
		gap: 12px;
		margin-top: 16px;
		align-items: center;
	}
</style>
