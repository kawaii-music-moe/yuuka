<script lang="ts">
// 連絡先の追加/編集モーダル（旧 app.js openContactModal + contact-form submit /
//  index.html #modal-contact）。editing が null なら新規。
// タグはカンマ区切り文字列で編集し、保存時に string[] へ分解する。

import type { ContactView } from "$lib/api/types";
import { Button, Modal } from "$lib/components/ui";

interface Props {
	open?: boolean;
	/** 編集対象。null なら新規追加。 */
	editing?: ContactView | null;
	/** 保存ハンドラ。親が API を叩き、成功時に open=false にする。 */
	onsave: (payload: {
		id?: number;
		name: string;
		birthday: string;
		relationship: string;
		contactInfo: string;
		notes: string;
		tags: string[];
	}) => void;
}

let { open = $bindable(false), editing = null, onsave }: Props = $props();

let name = $state("");
let birthday = $state("");
let relationship = $state("");
let contactInfo = $state("");
let tagsRaw = $state("");
let notes = $state("");

const isEdit = $derived(editing != null);

// 開くたびに editing で初期化（旧 openContactModal の value 代入）。
$effect(() => {
	if (!open) return;
	const c = editing;
	name = c?.name ?? "";
	birthday = c?.birthday ?? "";
	relationship = c?.relationship ?? "";
	contactInfo = c?.contact_info ?? "";
	tagsRaw = (c?.tags ?? []).join(", ");
	notes = c?.notes ?? "";
});

function submit(e: SubmitEvent) {
	e.preventDefault();
	const trimmed = name.trim();
	if (!trimmed) return;
	const tags = tagsRaw
		.split(",")
		.map((t) => t.trim())
		.filter((t) => t.length > 0);
	onsave({
		id: editing?.id,
		name: trimmed,
		birthday: birthday.trim(),
		relationship: relationship.trim(),
		contactInfo: contactInfo.trim(),
		notes: notes.trim(),
		tags,
	});
}
</script>

<Modal
	bind:open
	title={isEdit ? `連絡先の編集: ${editing?.name}` : "連絡先の追加"}
>
	<form onsubmit={submit}>
		<div class="form-group">
			<label for="contact-name">氏名 *</label>
			<input
				type="text"
				id="contact-name"
				required
				placeholder="例: 山田 太郎"
				bind:value={name}
			/>
		</div>
		<div class="form-row">
			<div class="form-group">
				<label for="contact-birthday">誕生日</label>
				<input
					type="text"
					id="contact-birthday"
					placeholder="1990-04-01 または --04-01"
					bind:value={birthday}
				/>
				<span class="field-sub">※年不明の場合は --MM-DD 形式</span>
			</div>
			<div class="form-group">
				<label for="contact-relationship">関係</label>
				<input
					type="text"
					id="contact-relationship"
					placeholder="例: 友人, 同僚"
					bind:value={relationship}
				/>
			</div>
		</div>
		<div class="form-group">
			<label for="contact-info">連絡先情報</label>
			<input
				type="text"
				id="contact-info"
				placeholder="例: example@example.com / 090-xxxx-xxxx"
				bind:value={contactInfo}
			/>
		</div>
		<div class="form-group">
			<label for="contact-tags">タグ (カンマ区切り)</label>
			<input
				type="text"
				id="contact-tags"
				placeholder="例: 大学, ゼミ"
				bind:value={tagsRaw}
			/>
		</div>
		<div class="form-group">
			<label for="contact-notes">メモ</label>
			<textarea id="contact-notes" placeholder="自由メモ" bind:value={notes}
			></textarea>
		</div>
		<Button type="submit" variant="primary" block>連絡先を保存</Button>
	</form>
</Modal>
