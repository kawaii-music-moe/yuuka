<script lang="ts">
// Webhook エンドポイントの作成フォーム。Playbook の登録フォームと同じく、一覧の代わりに
// ページ内へ出す。作成のみ（旧仕様どおり）。保存は親の onsave、取り消しは oncancel へ委譲。
import { Button, Icon } from "$lib/components/ui";
import TargetField from "../delivery/TargetField.svelte";

interface Props {
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
	oncancel: () => void;
}

let { onsave, oncancel }: Props = $props();

// 開くたびに作り直されるため、初期化は宣言時の値だけでよい。
let name = $state("");
let secret = $state("");
let notifyType = $state<"dm" | "channel">("dm");
let notifyId = $state("");
let template = $state("");
let filter = $state("");
let createTodo = $state(false);
let createReminder = $state(false);

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

<div class="action-column card">
	<div class="column-header">
		<h3><Icon name="webhook" class="header-icon-symbol" />Webhook エンドポイントの作成</h3>
	</div>
	<form class="form-stack" onsubmit={submit}>
		<div class="form-row">
			<div class="form-group">
				<label for="webhook-name">名前<span class="required-mark"> *</span></label>
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
				<!-- type="password" だとブラウザやパスワードマネージャーが保存済みパスワードを
				     候補に出すため、text にして表示だけ伏せ字にする。 -->
				<input
					type="text"
					id="webhook-secret"
					class="secret-input"
					placeholder="HMAC-SHA256 署名検証に使用"
					autocomplete="off"
					spellcheck="false"
					data-1p-ignore
					data-lpignore="true"
					data-bwignore
					bind:value={secret}
				/>
			</div>
		</div>
		<div class="form-group">
			<label for="webhook-notify-type">通知先</label>
			<TargetField
				id="webhook-notify-type"
				idInputId="webhook-notify-id"
				bind:type={notifyType}
				bind:targetId={notifyId}
			/>
		</div>
		<div class="form-group">
			<label for="webhook-template">通知テンプレート (任意)</label>
			<textarea
				id="webhook-template"
				rows="3"
				placeholder={"例: {{repository.name}} で {{action}} がありました"}
				bind:value={template}
			></textarea>
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
		<label class="check-row">
			<input type="checkbox" class="checkbox-custom" bind:checked={createTodo} />
			<span>受信時にToDoを自動作成する</span>
		</label>
		<label class="check-row">
			<input type="checkbox" class="checkbox-custom" bind:checked={createReminder} />
			<span>受信時にリマインダーを自動作成する</span>
		</label>
		<Button type="submit" variant="primary" block>Webhookを作成</Button>
		<Button variant="secondary" block onclick={oncancel}>キャンセル</Button>
	</form>
</div>
