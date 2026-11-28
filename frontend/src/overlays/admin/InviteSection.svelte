<script lang="ts">
// 招待コード（REGISTRATION_MODE=invite）。発行・一覧・削除と、招待リンクのコピー。
// docs/design/optional_discord.md §4.1。
import { onMount } from "svelte";
import { adminApi } from "$lib/api/services";
import type { AdminInviteView } from "$lib/api/types";
import { Button, confirmDialog, Icon } from "$lib/components/ui";
import { withBasePath } from "$lib/router";
import { pushToast, toastError } from "$lib/stores/toast";

let invites = $state<AdminInviteView[]>([]);
let note = $state("");
let expiresInDays = $state<number | null>(7);
let creating = $state(false);

async function load(): Promise<void> {
	try {
		invites = (await adminApi.invites()).invites ?? [];
	} catch (e) {
		toastError(e);
	}
}

onMount(load);

function inviteLink(code: string): string {
	const path = withBasePath("/login");
	const query = `invite=${encodeURIComponent(code)}`;
	return `${window.location.origin}${path}?${query}`;
}

async function copyLink(code: string): Promise<void> {
	try {
		await navigator.clipboard.writeText(inviteLink(code));
		pushToast("招待リンクをコピーしました。", "success");
	} catch {
		pushToast(inviteLink(code), "info");
	}
}

async function create(event: SubmitEvent): Promise<void> {
	event.preventDefault();
	creating = true;
	try {
		const res = await adminApi.createInvite({
			note: note.trim() || undefined,
			expiresInDays: expiresInDays ?? undefined,
		});
		note = "";
		await load();
		if (res.invite) await copyLink(res.invite.code);
	} catch (e) {
		toastError(e);
	} finally {
		creating = false;
	}
}

async function remove(invite: AdminInviteView): Promise<void> {
	const ok = await confirmDialog({
		message: `招待コード「${invite.code}」を削除しますか？`,
		danger: true,
		confirmLabel: "削除",
	});
	if (!ok) return;
	try {
		await adminApi.deleteInvite(invite.code);
		await load();
	} catch (e) {
		toastError(e);
	}
}

function status(invite: AdminInviteView): string {
	if (invite.usedAt) return `使用済み（${invite.usedAt}）`;
	if (invite.expiresAt) return `未使用（期限 ${invite.expiresAt}）`;
	return "未使用（期限なし）";
}
</script>

<p class="admin-section-note">
	新規登録が招待制（<code>REGISTRATION_MODE=invite</code>）のとき、初めてログインする人には招待リンクを送ります。コードは 1 回だけ使えます。
</p>
<form class="admin-form-top" onsubmit={create}>
	<div class="admin-inline-form">
		<input
			type="text"
			id="admin-invite-note"
			bind:value={note}
			maxlength="200"
			placeholder="メモ（誰に送るか など・任意）"
			class="admin-grow"
		/>
		<input
			type="number"
			id="admin-invite-expires"
			bind:value={expiresInDays}
			min="1"
			max="365"
			placeholder="期限（日）"
			aria-label="有効期限（日）"
		/>
		<Button variant="primary" type="submit" disabled={creating}>招待コードを発行</Button>
	</div>
</form>
<div class="table-responsive">
	<table class="expense-table admin-table-full">
		<thead>
			<tr>
				<th class="admin-table-th">コード</th>
				<th class="admin-table-th">メモ</th>
				<th class="admin-table-th">状態</th>
				<th class="admin-table-th">発行日</th>
				<th class="admin-table-th admin-th-actions">操作</th>
			</tr>
		</thead>
		<tbody>
			{#if invites.length === 0}
				<tr><td colspan="5" class="admin-empty">招待コードはありません。</td></tr>
			{:else}
				{#each invites as invite (invite.code)}
					<tr>
						<td class="admin-table-td"><code>{invite.code}</code></td>
						<td class="admin-table-td">{invite.note ?? ""}</td>
						<td class="admin-table-td">{status(invite)}</td>
						<td class="admin-table-td admin-td-date">{invite.createdAt}</td>
						<td class="admin-table-td admin-td-actions">
							<div class="admin-action-group">
								{#if !invite.usedAt}
									<button
										type="button"
										class="icon-plain"
										aria-label="招待リンクをコピー"
										title="招待リンクをコピー"
										onclick={() => copyLink(invite.code)}
									>
										<Icon name="content_copy" />
									</button>
								{/if}
								<button
									type="button"
									class="icon-plain danger"
									aria-label="招待コード「{invite.code}」を削除"
									title="削除"
									onclick={() => remove(invite)}
								>
									<Icon name="delete" />
								</button>
							</div>
						</td>
					</tr>
				{/each}
			{/if}
		</tbody>
	</table>
</div>
