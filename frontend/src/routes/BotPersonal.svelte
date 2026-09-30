<script lang="ts">
// ─────────────────────────────────────────────────────────────────────────
// パーソナル タブ（旧 app.js の context-note / clipboard / contacts + index.html
// #tab-personal を移植）。activeBot 変更で3本 fetch（fetchContextNote /
// fetchClipboardList / fetchContactsList）。personalApi 使用（scope:'bot'）。
// ─────────────────────────────────────────────────────────────────────────

import { ApiError } from "$lib/api/client";
import { personalApi } from "$lib/api/services";
import type { ClipboardEntry, ContactView } from "$lib/api/types";
import {
	Button,
	CharCounter,
	confirmDialog,
	Icon,
	TagChip,
} from "$lib/components/ui";
import { activeBot } from "$lib/stores/activeBot";
import { pushToast } from "$lib/stores/toast";
import ContactModal from "./personal/ContactModal.svelte";

// ── コンテキストノート ──
let noteContent = $state("");
let noteMaxLength = $state(10000);

// ── クリップボード ──
let clipboard = $state<ClipboardEntry[]>([]);

// ── 連絡先 ──
let contacts = $state<ContactView[]>([]);

// ── 連絡先モーダル ──
let contactOpen = $state(false);
let editingContact = $state<ContactView | null>(null);

function reportError(e: unknown) {
	pushToast(
		e instanceof ApiError ? e.message : "エラーが発生しました",
		"error",
	);
}

async function loadContextNote() {
	try {
		const res = await personalApi.getContextNote();
		noteMaxLength = res.max_length || 10000;
		noteContent = res.content ?? "";
	} catch (e) {
		reportError(e);
	}
}

async function loadClipboard() {
	try {
		const res = await personalApi.clipboard();
		clipboard = res.entries ?? [];
	} catch (e) {
		reportError(e);
		clipboard = [];
	}
}

async function loadContacts() {
	try {
		const res = await personalApi.contacts();
		contacts = res.contacts ?? [];
	} catch (e) {
		reportError(e);
		contacts = [];
	}
}

// activeBot（bot-scoped）変更で3本再取得。
$effect(() => {
	void $activeBot?.id;
	void loadContextNote();
	void loadClipboard();
	void loadContacts();
});

async function saveNote(e: SubmitEvent) {
	e.preventDefault();
	if (noteContent.length > noteMaxLength) {
		pushToast(
			`コンテキストノートは最大 ${noteMaxLength.toLocaleString()} 文字までです。`,
			"error",
		);
		return;
	}
	try {
		const res = await personalApi.saveContextNote(noteContent);
		pushToast(res.message ?? "保存しました。", "success");
	} catch (e) {
		reportError(e);
	}
}

async function deleteClip(id: number) {
	const ok = await confirmDialog({
		message: "このメモを削除しますか？",
		danger: true,
		confirmLabel: "削除",
	});
	if (!ok) return;
	try {
		await personalApi.deleteClipboard(id);
		await loadClipboard();
	} catch (e) {
		reportError(e);
	}
}

function openNewContact() {
	editingContact = null;
	contactOpen = true;
}
function openEditContact(c: ContactView) {
	editingContact = c;
	contactOpen = true;
}

async function saveContact(payload: {
	id?: number;
	name: string;
	birthday: string;
	relationship: string;
	contactInfo: string;
	notes: string;
	tags: string[];
}) {
	try {
		const res = await personalApi.saveContact(payload);
		pushToast(res.message ?? "保存しました。", "success");
		contactOpen = false;
		await loadContacts();
	} catch (e) {
		reportError(e);
	}
}

async function deleteContactRow(c: ContactView) {
	const ok = await confirmDialog({
		message: `連絡先「${c.name}」を削除しますか？`,
		danger: true,
		confirmLabel: "削除",
	});
	if (!ok) return;
	try {
		await personalApi.deleteContact(c.id);
		await loadContacts();
	} catch (e) {
		reportError(e);
	}
}
</script>

<section class="tab-view">
	<!-- コンテキストノート -->
	<div class="card">
		<div class="column-header">
			<h3>
				<Icon name="sticky_note_2" class="header-icon-symbol" />コンテキストノート
				(長期メモ)
			</h3>
			<CharCounter value={noteContent} max={noteMaxLength} class="hud-tag" />
		</div>
		<p class="description-text">
			AIが会話のたびに参照する長期メモです。好み・習慣・覚えておいてほしいことを自由に記述できます。
		</p>
		<form class="context-note-form" onsubmit={saveNote}>
			<textarea
				class="context-note-textarea"
				placeholder={"例: 私は猫アレルギーがあります。\n毎週火曜は燃えるゴミの日。"}
				bind:value={noteContent}
			></textarea>
			<Button type="submit" variant="primary" class="context-note-save"
				>ノートを保存</Button
			>
		</form>
	</div>

	<!-- クリップボード -->
	<div class="card personal-card-spacer">
		<div class="column-header">
			<h3>
				<Icon name="content_paste" class="header-icon-symbol" />クリップボード
				(一時メモ)
			</h3>
		</div>
		<p class="description-text">
			Discordで「メモして」と頼んだ一時メモです。期限が切れると自動削除されます。
		</p>
		<div class="clipboard-list">
			{#if clipboard.length > 0}
				{#each clipboard as item (item.id)}
					<div class="clipboard-item">
						<div class="clipboard-item-body">
							<div class="clipboard-content">{item.content}</div>
							<div class="clipboard-expires">
								{item.expires_at ? `期限: ${item.expires_at}` : "無期限"}
							</div>
						</div>
						<button
							type="button"
							class="btn-trash"
							aria-label="削除"
							onclick={() => deleteClip(item.id)}
						>
							<Icon name="delete" />
						</button>
					</div>
				{/each}
			{:else}
				<p class="personal-empty">クリップボードは空です。</p>
			{/if}
		</div>
	</div>

	<!-- 連絡先 -->
	<div class="card personal-card-spacer">
		<div class="column-header action-right">
			<h3><Icon name="contacts" class="header-icon-symbol" />連絡先</h3>
			<Button variant="primary" small onclick={openNewContact}>＋ 連絡先追加</Button>
		</div>
		<p class="description-text">
			誕生日を登録すると、当日にお祝いリマインドが自動配信されます。
		</p>
		<div class="table-responsive contacts-table-wrap">
			<table class="expense-table">
				<thead>
					<tr>
						<th>氏名</th>
						<th>誕生日</th>
						<th>関係</th>
						<th>連絡先</th>
						<th>タグ</th>
						<th class="contacts-actions-th">操作</th>
					</tr>
				</thead>
				<tbody>
					{#if contacts.length > 0}
						{#each contacts as c (c.id)}
							<tr>
								<td class="contacts-name">{c.name}</td>
								<td class="contacts-mono">{c.birthday || "—"}</td>
								<td>{c.relationship || "—"}</td>
								<td class="contacts-info">{c.contact_info || "—"}</td>
								<td>
									{#if c.tags.length > 0}
										{#each c.tags as tag (tag)}
											<TagChip label={`#${tag}`} />
										{/each}
									{:else}
										—
									{/if}
								</td>
								<td class="contacts-actions-td">
									<button
										type="button"
										class="btn btn-secondary btn-sm contacts-mini-btn"
										onclick={() => openEditContact(c)}>編集</button
									>
									<button
										type="button"
										class="btn btn-secondary btn-sm contacts-mini-btn"
										aria-label="削除"
										onclick={() => deleteContactRow(c)}
									>
										<Icon name="delete" size={13} />
									</button>
								</td>
							</tr>
						{/each}
					{:else}
						<tr>
							<td colspan="6" class="contacts-empty">連絡先は登録されていません。</td>
						</tr>
					{/if}
				</tbody>
			</table>
		</div>
	</div>
</section>

<ContactModal bind:open={contactOpen} editing={editingContact} onsave={saveContact} />

<style>
	.context-note-form {
		display: flex;
		flex-direction: column;
		gap: 12px;
		margin-top: 12px;
	}
	.context-note-textarea {
		min-height: 180px;
		font-size: 0.88rem;
		line-height: 1.6;
	}
	:global(.context-note-save) {
		align-self: flex-start;
	}
	.personal-card-spacer {
		margin-top: 24px;
	}
	.clipboard-list {
		display: flex;
		flex-direction: column;
		gap: 8px;
		margin-top: 12px;
	}
	.clipboard-item {
		display: flex;
		justify-content: space-between;
		gap: 10px;
		padding: 8px 12px;
		border: 1px solid var(--border-divider);
		border-radius: var(--radius);
	}
	.clipboard-item-body {
		flex: 1;
	}
	.clipboard-content {
		font-size: 0.85rem;
		word-break: break-all;
	}
	.clipboard-expires {
		font-size: 0.72rem;
		color: var(--color-zinc-muted);
	}
	.personal-empty {
		font-size: 0.8rem;
		color: var(--color-zinc-muted);
	}
	.contacts-table-wrap {
		margin-top: 12px;
	}
	.contacts-name {
		font-weight: 600;
	}
	.contacts-mono {
		font-family: var(--font-family-mono);
		font-size: 0.8rem;
	}
	.contacts-info {
		font-size: 0.8rem;
	}
	.contacts-actions-th {
		text-align: right;
		width: 110px;
	}
	.contacts-actions-td {
		text-align: right;
		white-space: nowrap;
	}
	.contacts-mini-btn {
		font-size: 0.72rem;
	}
	.contacts-empty {
		text-align: center;
		font-size: 0.8rem;
		color: var(--color-zinc-muted);
	}
</style>
