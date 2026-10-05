<script lang="ts">
// ─────────────────────────────────────────────────────────────────────────
// ペルソナ タブ（旧 app.js persona 系 + index.html #tab-personas を移植）。
//   - マイペルソナ一覧（適用中ID をハイライト）: GET /api/personas
//   - マーケットプレイス一覧: GET /api/personas/marketplace
//   - 全文プレビュー: GET /api/personas/marketplace/:id → プレビューモーダル
//   - 作成/編集/削除/適用/公開/インポート
// personaApi 使用。list/activate は scope:'bot'（適用は Bot 単位）、他は scope:'user'。
// （旧タブにある「汎用モード Bot単位ペルソナ」「共有時の推奨ペルソナ」カードは
//   botAttribute/bot API 依存で本グループ範囲外のため含めない。）
// ─────────────────────────────────────────────────────────────────────────

import { ApiError } from "$lib/api/client";
import { personaApi } from "$lib/api/services";
import type { PersonaRecord, PublicPersonaView } from "$lib/api/types";
import {
	Badge,
	Button,
	confirmDialog,
	EmptyState,
	Icon,
} from "$lib/components/ui";
import { activeBot } from "$lib/stores/activeBot";
import { pushToast } from "$lib/stores/toast";
import PersonaEditModal from "./personas/PersonaEditModal.svelte";
import PersonaPreviewModal from "./personas/PersonaPreviewModal.svelte";

let personas = $state<PersonaRecord[]>([]);
let activePersonaId = $state<number | null>(null);
let maxLength = $state(20000);
let marketplace = $state<PublicPersonaView[]>([]);

// 編集モーダル
let editOpen = $state(false);
let editingPersona = $state<PersonaRecord | null>(null);

// プレビューモーダル
let previewOpen = $state(false);
let previewName = $state("");
let previewPrompt = $state("");
let previewId = $state<number | null>(null);

const activeName = $derived(
	personas.find((p) => p.id === activePersonaId)?.name ?? "デフォルトペルソナ",
);

function reportError(e: unknown) {
	pushToast(
		e instanceof ApiError ? e.message : "エラーが発生しました",
		"error",
	);
}

async function loadPersonas() {
	try {
		const res = await personaApi.list();
		personas = res.personas ?? [];
		activePersonaId = res.active_persona_id ?? null;
		maxLength = res.max_length || 20000;
	} catch (e) {
		reportError(e);
		personas = [];
	}
}

async function loadMarketplace() {
	try {
		const res = await personaApi.marketplace();
		marketplace = res.personas ?? [];
	} catch (e) {
		reportError(e);
		marketplace = [];
	}
}

// activeBot（適用中ID が Bot 単位）変更で再取得。
$effect(() => {
	void $activeBot?.id;
	void loadPersonas();
	void loadMarketplace();
});

async function reloadAll() {
	await loadPersonas();
	await loadMarketplace();
}

function openNew() {
	editingPersona = null;
	editOpen = true;
}
function openEdit(p: PersonaRecord) {
	editingPersona = p;
	editOpen = true;
}

async function savePersona(payload: {
	id?: number;
	name: string;
	prompt: string;
}) {
	try {
		const res = await personaApi.save(payload);
		pushToast(res.message ?? "保存しました。", "success");
		editOpen = false;
		await loadPersonas();
	} catch (e) {
		reportError(e);
	}
}

async function activate(id: number | null) {
	if (id == null) {
		const ok = await confirmDialog({
			message: "デフォルトペルソナに戻しますか？",
			confirmLabel: "戻す",
		});
		if (!ok) return;
	}
	try {
		const res = await personaApi.activate(id);
		pushToast(res.message ?? "適用しました。", "success");
		await reloadAll();
	} catch (e) {
		reportError(e);
	}
}

async function togglePublish(p: PersonaRecord) {
	const willPublish = p.is_public !== 1;
	const ok = await confirmDialog({
		message: willPublish
			? `ペルソナ「${p.name}」をマーケットプレイスに公開しますか？\n全ユーザーが内容を閲覧・インポートできるようになります。`
			: `ペルソナ「${p.name}」を非公開化しますか？`,
		confirmLabel: willPublish ? "公開する" : "非公開にする",
	});
	if (!ok) return;
	try {
		const res = await personaApi.publish(p.id, willPublish);
		pushToast(res.message ?? "更新しました。", "success");
		await reloadAll();
	} catch (e) {
		reportError(e);
	}
}

async function deletePersona(p: PersonaRecord) {
	const ok = await confirmDialog({
		message: `ペルソナ「${p.name}」を削除しますか？`,
		danger: true,
		confirmLabel: "削除",
	});
	if (!ok) return;
	try {
		await personaApi.delete(p.id);
		await reloadAll();
	} catch (e) {
		reportError(e);
	}
}

async function openPreview(m: PublicPersonaView) {
	try {
		const res = await personaApi.marketplaceDetail(m.id);
		previewName = res.persona.name;
		previewPrompt = res.persona.prompt;
		previewId = res.persona.id;
		previewOpen = true;
	} catch (e) {
		reportError(e);
	}
}

async function importPersona(id: number) {
	try {
		const res = await personaApi.import(id);
		pushToast(res.message ?? "インポートしました。", "success");
		previewOpen = false;
		await loadPersonas();
	} catch (e) {
		reportError(e);
	}
}
</script>

<section class="tab-view">
	<div class="view-actions-card card">
		<div class="filters-group">
			<span class="label-text">適用中: {activeName}</span>
		</div>
		<div class="personas-actions">
			<Button variant="secondary" onclick={() => activate(null)}>デフォルトに戻す</Button>
			<Button variant="primary" onclick={openNew}>＋ ペルソナ作成</Button>
		</div>
	</div>

	<!-- マイペルソナ -->
	<div class="card personas-card-spacer">
		<div class="column-header">
			<h3><Icon name="theater_comedy" class="header-icon-symbol" />マイペルソナ</h3>
		</div>
		<p class="description-text">
			作成したペルソナの一覧です。「適用」すると会話に反映されます。「公開」するとマーケットプレイスに掲載されます。
		</p>
		<div class="list-container personas-list">
			{#if personas.length > 0}
				{#each personas as p (p.id)}
					<div class="card-item glass persona-item">
						<div class="persona-item-head">
							<div class="persona-item-title">
								<span class="persona-name">{p.name}</span>
								{#if p.id === activePersonaId}
									<Badge tone="status-sent">適用中</Badge>
								{/if}
								{#if p.is_public === 1}
									<Badge tone="status-pending">公開中</Badge>
								{/if}
							</div>
							<span class="persona-len">{p.prompt.length.toLocaleString()} 文字</span>
						</div>
						<div class="persona-preview">{p.prompt.slice(0, 120)}</div>
						<div class="persona-item-actions">
							{#if p.id !== activePersonaId}
								<button
									type="button"
									class="btn-mini"
									onclick={() => activate(p.id)}
								>
									<Icon name="check" />適用
								</button>
							{/if}
							<button type="button" class="btn-mini" onclick={() => openEdit(p)}>
								<Icon name="edit" />編集
							</button>
							<button type="button" class="btn-mini" onclick={() => togglePublish(p)}>
								<Icon name={p.is_public === 1 ? "lock" : "public"} />
								{p.is_public === 1 ? "非公開にする" : "公開する"}
							</button>
							<button
								type="button"
								class="btn-mini"
								onclick={() => deletePersona(p)}
							>
								<Icon name="delete" />削除
							</button>
						</div>
					</div>
				{/each}
			{:else}
				<EmptyState
					icon="theater_comedy"
					message="ペルソナが作成されていません。「＋ ペルソナ作成」から作成できます。"
				/>
			{/if}
		</div>
	</div>

	<!-- マーケットプレイス -->
	<div class="card personas-card-spacer">
		<div class="column-header badge-right">
			<h3>
				<Icon name="storefront" class="header-icon-symbol" />ペルソナ マーケットプレイス
			</h3>
			<span class="badge badge-accent">公開ペルソナ</span>
		</div>
		<p class="description-text">
			他のユーザーが公開しているペルソナです。インポートすると自分のペルソナとして独立コピーされます。
		</p>
		<div class="list-container personas-list">
			{#if marketplace.length > 0}
				{#each marketplace as m (m.id)}
					<div class="card-item glass persona-item">
						<div class="persona-item-head">
							<span class="persona-name">{m.name}</span>
							<span class="persona-len">
								by {m.owner_username} ・ {m.prompt_length.toLocaleString()} 文字
							</span>
						</div>
						<div class="persona-preview">{m.prompt_preview}</div>
						<div class="persona-item-actions">
							<button type="button" class="btn-mini" onclick={() => openPreview(m)}>
								<Icon name="visibility" />全文表示
							</button>
							<button type="button" class="btn-mini" onclick={() => importPersona(m.id)}>
								<Icon name="download" />インポート
							</button>
						</div>
					</div>
				{/each}
			{:else}
				<EmptyState
					icon="storefront"
					message="公開されているペルソナはまだありません。"
				/>
			{/if}
		</div>
	</div>
</section>

<PersonaEditModal
	bind:open={editOpen}
	editing={editingPersona}
	{maxLength}
	onsave={savePersona}
/>
<PersonaPreviewModal
	bind:open={previewOpen}
	personaName={previewName}
	prompt={previewPrompt}
	personaId={previewId}
	onimport={importPersona}
/>

<style>
	.personas-actions {
		display: flex;
		gap: 10px;
	}
	.personas-card-spacer {
		margin-top: 20px;
	}
	.personas-list {
		margin-top: 12px;
	}
	.persona-item {
		flex-direction: column;
		align-items: stretch;
		gap: 8px;
	}
	.persona-item-head {
		display: flex;
		justify-content: space-between;
		align-items: center;
		gap: 8px;
	}
	.persona-item-title {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
	}
	.persona-name {
		font-weight: 600;
	}
	.persona-len {
		font-size: 0.75rem;
		color: var(--color-zinc-muted);
		white-space: nowrap;
	}
	.persona-preview {
		font-size: 0.82rem;
		color: var(--color-zinc-muted);
		line-height: 1.5;
		word-break: break-word;
	}
	.persona-item-actions {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
	}
</style>
