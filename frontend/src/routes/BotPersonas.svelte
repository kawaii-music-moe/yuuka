<script lang="ts">
// ─────────────────────────────────────────────────────────────────────────
// 人格（Bot設定 › このBot。旧 app.js persona 系 + index.html #tab-personas を移植）。
//   - マイペルソナ一覧（適用中ID をハイライト）: GET /api/personas
//   - マーケットプレイス一覧: GET /api/personas/marketplace
//   - 全文プレビュー: GET /api/personas/marketplace/:id → プレビューモーダル
//   - 作成/編集/削除/適用/公開/インポート
//   - 危険な操作: 会話履歴のクリア（再構成で旧 統合管理 › Bot ヘルスから移した）
// personaApi 使用。list/activate は scope:'bot'（適用は Bot 単位）、他は scope:'user'。
// （旧タブにある「汎用モード Bot単位ペルソナ」「共有時の推奨ペルソナ」カードは
//   botAttribute/bot API 依存で本グループ範囲外のため含めない。）
// ─────────────────────────────────────────────────────────────────────────

import { ApiError } from "$lib/api/client";
import { integratedApi, personaApi } from "$lib/api/services";
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
import PersonaAddModal from "./personas/PersonaAddModal.svelte";
import PersonaEditModal from "./personas/PersonaEditModal.svelte";
import PersonaPreviewModal from "./personas/PersonaPreviewModal.svelte";

let personas = $state<PersonaRecord[]>([]);
let activePersonaId = $state<number | null>(null);
let maxLength = $state(20000);
let marketplace = $state<PublicPersonaView[]>([]);

// 追加モーダル（自分で作成 / マーケットから追加）
let addOpen = $state(false);

// 編集モーダル
let editOpen = $state(false);
let editingPersona = $state<PersonaRecord | null>(null);

// プレビューモーダル
let previewOpen = $state(false);
let previewName = $state("");
let previewPrompt = $state("");
let previewId = $state<number | null>(null);

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

function openAdd() {
	addOpen = true;
}
function openNew() {
	editingPersona = null;
	addOpen = false;
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
	const willPublish = !p.is_public;
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
		addOpen = false;
		await loadPersonas();
	} catch (e) {
		reportError(e);
	}
}

// ── 会話履歴クリア（旧 IntegratedOverlay の intClearHistory） ──
async function clearHistory() {
	const botId = $activeBot?.id;
	if (!botId) return;
	const ok = await confirmDialog({
		message:
			"このBotとの会話履歴をクリアしますか？\n次のメッセージから新しい会話になります（永続ログは保持され、検索・監査では引き続き利用できます）。",
		danger: true,
		confirmLabel: "クリア",
	});
	if (!ok) return;
	try {
		await integratedApi.clearHistory(botId);
		pushToast("会話履歴をクリアしました。", "success");
	} catch (e) {
		reportError(e);
	}
}
</script>

<section class="tab-view">
	<!-- マイペルソナ -->
	<div class="card personas-card-spacer">
		<div class="column-header badge-right">
			<h3><Icon name="theater_comedy" class="header-icon-symbol" />マイペルソナ</h3>
			<div class="personas-actions">
				<Button
					variant="icon"
					title="デフォルトに戻す"
					aria-label="デフォルトに戻す"
					onclick={() => activate(null)}
				>
					<Icon name="restart_alt" />
				</Button>
				<Button
					variant="icon"
					title="ペルソナを追加"
					aria-label="ペルソナを追加"
					onclick={openAdd}
				>
					<Icon name="add" />
				</Button>
			</div>
		</div>
		<div class="list-container personas-list">
			{#if personas.length > 0}
				{#each personas as p (p.id)}
					<div class="card-item glass persona-row">
						<div class="persona-row-head">
							<div class="entry-title">
								<span class="entry-name">{p.name}</span>
								{#if p.id === activePersonaId}
									<Badge tone="status-sent">適用中</Badge>
							{/if}
								{#if p.is_public}
									<Badge tone="status-pending">公開中</Badge>
							{/if}
							</div>
							<span class="persona-row-meta">{p.prompt.length.toLocaleString()} 文字</span>
						</div>
						<div class="persona-row-preview">{p.prompt.slice(0, 120)}</div>
						<div class="persona-row-actions">
							{#if p.id !== activePersonaId}
								<Button
									variant="icon"
									title="適用"
									aria-label="適用"
									onclick={() => activate(p.id)}
								>
									<Icon name="check" />
								</Button>
							{/if}
							<Button
								variant="icon"
								title="編集"
								aria-label="編集"
								onclick={() => openEdit(p)}
							>
								<Icon name="edit" />
							</Button>
							<Button
								variant="icon"
								title={p.is_public ? "公開中（クリックで非公開にする）" : "非公開（クリックで公開する）"}
								aria-label={p.is_public ? "公開中（クリックで非公開にする）" : "非公開（クリックで公開する）"}
								onclick={() => togglePublish(p)}
							>
								<Icon name={p.is_public ? "public" : "lock"} />
							</Button>
							<Button
								variant="icon"
								class="persona-action-delete"
								title="削除"
								aria-label="削除"
								onclick={() => deletePersona(p)}
							>
								<Icon name="delete" />
							</Button>
						</div>
					</div>
				{/each}
			{:else}
				<EmptyState
					icon="theater_comedy"
					message="ペルソナが作成されていません。右上の＋ボタンから作成、またはマーケットから追加できます。"
				/>
			{/if}
		</div>
	</div>

	<!-- 危険な操作: 会話履歴のクリア（旧 統合管理 › Bot ヘルスの ⋮ メニュー） -->
	{#if $activeBot}
		<div class="card personas-card-spacer">
			<div class="column-header badge-right">
				<h3><Icon name="history" class="header-icon-symbol" />会話履歴のクリア</h3>
			</div>
			<p class="description-text">
				このBotとのあなたの会話履歴をクリアし、次のメッセージから新しい会話にします。ペルソナを切り替えたあと、前の口調を引きずるときに使います。永続ログは保持され、検索・監査では引き続き利用できます。
			</p>
			<div class="personas-danger-actions">
				<Button variant="danger" onclick={clearHistory}>会話履歴をクリア</Button>
			</div>
		</div>
	{/if}
</section>

<PersonaEditModal
	bind:open={editOpen}
	editing={editingPersona}
	{maxLength}
	onsave={savePersona}
/>
<PersonaAddModal
	bind:open={addOpen}
	{marketplace}
	closeOnEsc={!previewOpen}
	oncreate={openNew}
	onpreview={openPreview}
	onimport={importPersona}
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
		margin-left: auto;
		gap: 8px;
	}
	.personas-card-spacer {
		margin-top: 20px;
	}
	.personas-danger-actions {
		margin-top: 12px;
	}
	.personas-list {
		margin-top: 12px;
	}
	.persona-row-actions :global(.btn-icon) {
		border-color: transparent;
		color: var(--color-zinc-muted);
		transition: color 0.18s ease;
	}
	.persona-row-actions :global(.btn-icon:hover) {
		background-color: transparent;
		color: var(--text-primary);
	}
	.persona-row-actions :global(.btn-icon .material-symbols-outlined) {
		font-size: 16px;
	}
	.persona-row-actions :global(.btn-icon.persona-action-delete:hover) {
		color: var(--color-error);
	}
</style>
