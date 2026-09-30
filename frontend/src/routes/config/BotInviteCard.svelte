<script lang="ts">
// Bot招待リンク・プロフィールURLカード（旧 updateBotInviteCard + btn-invite-sync）。
// application_id があればリンク表示、無ければ「今すぐ同期」を促す。

import { ApiError } from "$lib/api/client";
import { botAttributeApi } from "$lib/api/services";
import { Button } from "$lib/components/ui";
import { pushToast } from "$lib/stores/toast";
import CopyField from "./CopyField.svelte";
import { type BotAttrView, botInviteUrl, botProfileUrl } from "./configTypes";

interface Props {
	botId: string;
	bot: BotAttrView;
	/** 同期成功後に親のカード群・Bot一覧を再取得 */
	onsynced?: () => void;
}
let { botId, bot, onsynced }: Props = $props();

const appId = $derived(bot.discord_application_id ?? "");
const inviteUrl = $derived(appId ? botInviteUrl(appId) : "");
const profileUrl = $derived(appId ? botProfileUrl(appId) : "");

let syncing = $state(false);
async function sync() {
	syncing = true;
	try {
		const res = await botAttributeApi.syncDiscord(botId);
		if (res.success) {
			onsynced?.();
		} else {
			pushToast(
				res.message ??
					"同期に失敗しました。Botが起動しているか確認してください。",
				"error",
			);
		}
	} catch (err) {
		pushToast(
			err instanceof ApiError ? err.message : "通信エラーが発生しました。",
			"error",
		);
	} finally {
		syncing = false;
	}
}
</script>

<details class="config-card card">
	<summary class="column-header">
		<h3><span class="material-symbols-outlined header-icon-symbol">link</span>Bot 招待リンク・プロフィール</h3>
	</summary>
	<p class="description-text">
		このBotをサーバーへ追加するための導入リンクと、Discordプロフィールへのリンクです。
	</p>

	{#if appId}
		<div class="invite-fields">
			<CopyField
				label="導入リンク（サーバーに追加）"
				value={inviteUrl}
				sub="※チャンネル表示・メッセージ送信／履歴閲覧・埋め込みリンク・ファイル添付の権限を付与した状態で開きます。"
			/>
			<CopyField label="プロフィールURL" value={profileUrl} />
		</div>
	{:else}
		<div class="invite-empty">
			<span class="field-sub">
				導入リンクを生成するには、Discord Bot Token を設定してください。トークンを設定すると、Botを起動していなくても導入リンク・プロフィールURLを表示できます。
			</span>
			<Button variant="secondary" onclick={sync} disabled={syncing}>
				{syncing ? "同期中..." : "今すぐ同期"}
			</Button>
		</div>
	{/if}
</details>

<style>
	.invite-fields {
		display: flex;
		flex-direction: column;
		gap: 16px;
		margin-top: 16px;
	}
	.invite-empty {
		margin-top: 12px;
		display: flex;
		flex-direction: column;
		gap: 8px;
		align-items: flex-start;
	}
</style>
