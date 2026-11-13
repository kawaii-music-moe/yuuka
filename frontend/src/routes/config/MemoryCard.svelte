<script lang="ts">
// 記憶カード（docs/design/memory_retention.md §4-E）。
// Bot があなたについて覚えている長期記憶（評価を通った事実文）を一覧し、1 件ずつ消せる。
// 記憶は あなた × Bot のスコープなので、「あなたの設定」では Bot をここで選ぶ（activeBot は使わない）。
// 評価待ちの候補は内容を出さず件数だけ表示する（評価器が事実文に書き直すまでは生の発話のため）。

import { onMount } from "svelte";
import { botApi, memoryApi } from "$lib/api/services";
import type { MemoryItem } from "$lib/api/services/memoryApi";
import type { BotView } from "$lib/api/types";
import {
	Button,
	confirmDialog,
	EmptyState,
	Icon,
	Select,
	type SelectOption,
} from "$lib/components/ui";
import { activeBot } from "$lib/stores/activeBot";
import { pushToast, toastError } from "$lib/stores/toast";
import HelpTip from "../delivery/HelpTip.svelte";
import { isDefaultBot } from "./configTypes";

let bots = $state<BotView[]>([]);
let botId = $state("");
let memories = $state<MemoryItem[]>([]);
let pendingCount = $state(0);
let loading = $state(false);
let loaded = $state(false);
let deletingId = $state<number | null>(null);

const botOptions = $derived<SelectOption[]>(
	bots.map((b) => ({ value: b.id, label: b.name || b.id })),
);

const KIND_LABELS: Record<string, string> = {
	preference: "好み",
	fact: "事実",
	relationship: "人間関係",
	schedule: "予定・習慣",
	health: "健康",
	work: "仕事・学業",
	goal: "目標",
};

const DURABILITY_LABELS: Record<string, string> = {
	long: "長期",
	medium: "中期",
	short: "短期",
};

function kindLabel(kind: string | null): string {
	if (!kind) return "未分類";
	return KIND_LABELS[kind] ?? kind;
}

function durabilityLabel(d: string): string {
	return DURABILITY_LABELS[d] ?? d;
}

/** 'YYYY-MM-DD HH:MM:SS' を 'YYYY-MM-DD HH:MM' に。null は「未使用」。 */
function formatUsed(ts: string | null): string {
	if (!ts) return "未使用";
	return ts.length >= 16 ? ts.slice(0, 16) : ts;
}

/** 強度（0〜）を 5 段階の目盛りで。1.0 が評価直後の値、有用で +1.0、毎日 ×0.97。 */
function strengthLevel(s: number): number {
	if (s >= 4) return 5;
	if (s >= 2) return 4;
	if (s >= 1) return 3;
	if (s >= 0.5) return 2;
	return 1;
}

async function loadBots() {
	try {
		const res = await botApi.list();
		bots = (res.bots ?? []).filter((b) => !isDefaultBot(b.id));
		// 選択中の Bot があればそれを、無ければ先頭を既定にする。
		const current = $activeBot?.id;
		if (current && bots.some((b) => b.id === current)) botId = current;
		else if (bots.length > 0) botId = bots[0].id;
	} catch (err) {
		toastError(err, "Bot の一覧を読み込めませんでした。");
	}
}

async function load() {
	if (!botId) {
		memories = [];
		pendingCount = 0;
		loaded = true;
		return;
	}
	loading = true;
	try {
		const res = await memoryApi.list(botId);
		memories = res.memories ?? [];
		pendingCount = res.pendingCount ?? 0;
		loaded = true;
	} catch (err) {
		toastError(err, "記憶を読み込めませんでした。");
	} finally {
		loading = false;
	}
}

onMount(async () => {
	await loadBots();
	await load();
});

async function remove(item: MemoryItem) {
	const ok = await confirmDialog({
		title: "記憶を消す",
		message: `「${item.content}」を忘れさせますか？この操作は取り消せません。`,
		danger: true,
		confirmLabel: "忘れさせる",
	});
	if (!ok) return;
	deletingId = item.id;
	try {
		const res = await memoryApi.remove(botId, item.id);
		if (res.success) {
			memories = memories.filter((m) => m.id !== item.id);
			pushToast("記憶を消しました。", "success");
		} else {
			pushToast(
				"その記憶は見つかりませんでした（もう消えている可能性があります）。",
				"error",
			);
			await load();
		}
	} catch (err) {
		toastError(err, "記憶を消せませんでした。");
	} finally {
		deletingId = null;
	}
}
</script>

<section class="config-card card">
	<div class="column-header badge-right">
		<h3><span class="material-symbols-outlined header-icon-symbol">psychology</span>記憶</h3>
		<span class="badge badge-accent">Beta</span>
	</div>
	<p class="description-text">
		Bot があなたについて覚えていること（会話から残した短い事実）です。いつでも 1 件ずつ消せます。会話で「〇〇のことは忘れて」と頼んでも消せます。
	</p>

	<div class="memory-toolbar">
		<div class="form-group memory-bot">
			<div class="label-row">
				<label for="memory-bot">Bot</label>
				<HelpTip
					id="memory-bot-help"
					label="Bot ごとの記憶"
					text="記憶は あなた × Bot ごとに別々に持ちます。他の人の記憶は見えません。"
				/>
			</div>
			{#if botOptions.length > 0}
				<Select id="memory-bot" options={botOptions} bind:value={botId} onchange={() => void load()} />
			{:else}
				<div class="field-sub">使える Bot がありません。</div>
			{/if}
		</div>
		<Button variant="secondary" small onclick={() => void load()} disabled={loading || !botId}>
			<Icon name="refresh" />再読み込み
		</Button>
	</div>

	{#if loaded}
		<div class="memory-summary">
			<span>覚えていること: <strong>{memories.length}</strong> 件</span>
			<span class="memory-pending" title="会話から拾ったものの、まだ残すかどうかを判定していないもの。判定を通ったものだけがここに並びます。">
				評価待ち: <strong>{pendingCount}</strong> 件
			</span>
		</div>

		{#if memories.length === 0}
			<EmptyState icon="psychology" message="この Bot が覚えていることはまだありません。" />
		{:else}
			<ul class="memory-list">
				{#each memories as m (m.id)}
					<li class="memory-item">
						<div class="memory-body">
							<div class="memory-content">{m.content}</div>
							<div class="memory-meta">
								<span class="memory-tag">{kindLabel(m.kind)}</span>
								<span class="memory-tag memory-tag-sub">{durabilityLabel(m.durability)}</span>
								<span>最終利用: {formatUsed(m.last_used_at)}</span>
								<span class="memory-strength" title={`強度 ${m.strength.toFixed(2)}（想起 ${m.use_count} 回・返答に効いた ${m.useful_count} 回）`}>
									強度:
									<span class="memory-bars" aria-hidden="true">
										{#each [1, 2, 3, 4, 5] as n}
											<span class="memory-bar" class:on={n <= strengthLevel(m.strength)}></span>
										{/each}
									</span>
								</span>
							</div>
						</div>
						<button
							type="button"
							class="btn-trash"
							aria-label="この記憶を消す"
							title="この記憶を消す"
							disabled={deletingId === m.id}
							onclick={() => remove(m)}
						>
							<Icon name="delete" />
						</button>
					</li>
				{/each}
			</ul>
		{/if}
	{/if}
</section>

<style>
	.memory-toolbar {
		display: flex;
		align-items: flex-end;
		justify-content: space-between;
		gap: 12px;
		margin-top: 16px;
	}
	.memory-bot {
		flex: 1;
		max-width: 320px;
	}
	.memory-summary {
		display: flex;
		gap: 16px;
		margin-top: 12px;
		font-size: 0.85rem;
		color: var(--color-zinc-muted);
	}
	.memory-pending {
		cursor: help;
	}
	.memory-list {
		list-style: none;
		margin: 12px 0 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 8px;
	}
	.memory-item {
		display: flex;
		justify-content: space-between;
		gap: 10px;
		padding: 8px 12px;
		border: 1px solid var(--border-divider);
		border-radius: var(--radius);
	}
	.memory-body {
		flex: 1;
		min-width: 0;
	}
	.memory-content {
		font-size: 0.9rem;
		word-break: break-word;
	}
	.memory-meta {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 8px;
		margin-top: 4px;
		font-size: 0.72rem;
		color: var(--color-zinc-muted);
	}
	.memory-tag {
		padding: 1px 8px;
		border-radius: 999px;
		background: color-mix(in srgb, var(--color-primary) 18%, transparent);
		color: var(--color-primary);
	}
	.memory-tag-sub {
		background: var(--border-divider);
		color: inherit;
	}
	.memory-strength {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		cursor: help;
	}
	.memory-bars {
		display: inline-flex;
		gap: 2px;
	}
	.memory-bar {
		width: 5px;
		height: 10px;
		border-radius: 1px;
		background: var(--border-divider);
	}
	.memory-bar.on {
		background: var(--color-primary);
	}
</style>
