<script lang="ts">
// ペルソナ 作成/編集モーダル（旧 app.js persona-edit-form + index.html
//  #modal-persona-edit）。editing が null なら新規。

import type { PersonaRecord } from "$lib/api/types";
import { Button, CharCounter, Modal } from "$lib/components/ui";

interface Props {
	open?: boolean;
	editing?: PersonaRecord | null;
	maxLength?: number;
	onsave: (payload: { id?: number; name: string; prompt: string }) => void;
}

let {
	open = $bindable(false),
	editing = null,
	maxLength = 20000,
	onsave,
}: Props = $props();

let name = $state("");
let prompt = $state("");

const isEdit = $derived(editing != null);

$effect(() => {
	if (!open) return;
	name = editing?.name ?? "";
	prompt = editing?.prompt ?? "";
});

function submit(e: SubmitEvent) {
	e.preventDefault();
	const trimmed = name.trim();
	if (!trimmed) return;
	if (prompt.length > maxLength) return;
	onsave({ id: editing?.id, name: trimmed, prompt });
}
</script>

<Modal
	bind:open
	wide
	title={isEdit ? `ペルソナの編集: ${editing?.name}` : "ペルソナの作成"}
>
	<form class="persona-edit-form" onsubmit={submit}>
		<div class="form-group">
			<label for="persona-edit-name">ペルソナ名 *</label>
			<input
				type="text"
				id="persona-edit-name"
				required
				placeholder="例: 関西弁の秘書"
				bind:value={name}
			/>
		</div>
		<div class="form-group">
			<label for="persona-edit-prompt">ペルソナ定義（システムプロンプト指示） *</label>
			<textarea
				id="persona-edit-prompt"
				required
				class="persona-edit-prompt"
				placeholder="例: あなたは優秀なAIアシスタントです。常に丁寧な関西弁で答えてください。"
				bind:value={prompt}
			></textarea>
			<CharCounter value={prompt} max={maxLength} class="field-sub" />
		</div>
		<div class="persona-edit-actions">
			<Button variant="secondary" onclick={() => (open = false)}>キャンセル</Button>
			<Button type="submit" variant="primary">保存する</Button>
		</div>
	</form>
</Modal>

<style>
	.persona-edit-form {
		display: flex;
		flex-direction: column;
		gap: 16px;
	}
	.persona-edit-prompt {
		min-height: 320px;
		font-size: 0.9rem;
		line-height: 1.5;
		width: 100%;
		box-sizing: border-box;
		resize: vertical;
	}
	.persona-edit-actions {
		display: flex;
		justify-content: flex-end;
		gap: 10px;
	}
</style>
