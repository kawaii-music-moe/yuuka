<script lang="ts">
// Webhook エンドポイント作成モーダル（旧 index.html #modal-webhook + webhook-form）。
// 作成のみ（旧仕様どおり）。保存は親の onsave へ委譲。
import { Button, Modal } from "$lib/components/ui";

interface Props {
	open?: boolean;
	onsave: (payload: {
		name: string;
		secret: string;
		notifyTargetType: "dm" | "channel";
		notifyTargetId: string;
		template: string;
		filterKeyword: string;
		createTodo: boolean;
		createReminder: boolean;
	}) => void;
}

let { open = $bindable(false), onsave }: Props = $props();

let name = $state("");
let secret = $state("");
let notifyType = $state<"dm" | "channel">("dm");
let notifyId = $state("");
let template = $state("");
let filter = $state("");
let createTodo = $state(false);
let createReminder = $state(false);

// 開くたびに初期化。
$effect(() => {
	if (!open) return;
	name = "";
	secret = "";
	notifyType = "dm";
	notifyId = "";
	template = "";
	filter = "";
	createTodo = false;
	createReminder = false;
});

function submit(e: SubmitEvent) {
	e.preventDefault();
	const trimmed = name.trim();
	if (!trimmed) return;
	onsave({
		name: trimmed,
		secret: secret.trim(),
		notifyTargetType: notifyType,
		notifyTargetId: notifyId.trim(),
		template: template.trim(),
		filterKeyword: filter.trim(),
		createTodo,
		createReminder,
	});
}
</script>

<Modal bind:open title="Webhookエンドポイントの作成">
	<form onsubmit={submit}>
		<div class="form-group">
			<label for="webhook-name">名前 *</label>
			<input
				type="text"
				id="webhook-name"
				required
				placeholder="例: GitHub通知"
				bind:value={name}
			/>
		</div>
		<div class="form-group">
			<label for="webhook-secret">署名シークレット (任意)</label>
			<input
				type="password"
				id="webhook-secret"
				placeholder="HMAC-SHA256 署名検証に使用"
				autocomplete="new-password"
				bind:value={secret}
			/>
		</div>
		<div class="form-row">
			<div class="form-group">
				<label for="webhook-notify-type">通知先</label>
				<select id="webhook-notify-type" bind:value={notifyType}>
					<option value="dm">DM</option>
					<option value="channel">チャンネル</option>
				</select>
			</div>
			<div class="form-group">
				<label for="webhook-notify-id">チャンネルID (チャンネル選択時)</label>
				<input
					type="text"
					id="webhook-notify-id"
					placeholder="例: 123456789012345678"
					bind:value={notifyId}
				/>
			</div>
		</div>
		<div class="form-group">
			<label for="webhook-template">通知テンプレート (任意)</label>
			<input
				type="text"
				id="webhook-template"
				placeholder={"例: {{repository.name}} で {{action}} がありました"}
				bind:value={template}
			/>
		</div>
		<div class="form-group">
			<label for="webhook-filter">フィルタキーワード (任意)</label>
			<input
				type="text"
				id="webhook-filter"
				placeholder="含まれない受信は通知せずスキップ"
				bind:value={filter}
			/>
		</div>
		<label class="webhook-check-row">
			<input type="checkbox" class="checkbox-custom" bind:checked={createTodo} />
			<span>受信時にToDoを自動作成する</span>
		</label>
		<label class="webhook-check-row">
			<input type="checkbox" class="checkbox-custom" bind:checked={createReminder} />
			<span>受信時にリマインダーを自動作成する</span>
		</label>
		<Button type="submit" variant="primary" block>Webhookを作成</Button>
	</form>
</Modal>

<style>
	.webhook-check-row {
		display: flex;
		align-items: center;
		gap: 10px;
		font-size: 0.85rem;
		margin-bottom: 12px;
	}
</style>
