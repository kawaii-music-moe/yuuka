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
// 誕生日は MM/DD で入力する。保存は "--MM-DD"（年は持たない）だが、
// 年付き（"YYYY-MM-DD"）で登録済みのものは年を消さずに残す。
let birthdayMd = $state("");
let birthdayYear = $state("");
let birthdayEl: HTMLInputElement | undefined = $state();
let pickerEl: HTMLInputElement | undefined = $state();
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
	const bday = /^(\d{4}|-)-(\d{2})-(\d{2})$/.exec(c?.birthday ?? "");
	birthdayYear = bday && bday[1] !== "-" ? bday[1] : "";
	birthdayMd = bday ? `${bday[2]}/${bday[3]}` : (c?.birthday ?? "");
	relationship = c?.relationship ?? "";
	contactInfo = c?.contact_info ?? "";
	tagsRaw = (c?.tags ?? []).join(", ");
	notes = c?.notes ?? "";
});

const pad2 = (n: number) => String(n).padStart(2, "0");

// "4/1" や "04/01" を [月, 日] にする。形式違い・存在しない日付は null。
function parseMonthDay(text: string): [number, number] | null {
	const m = /^(\d{1,2})\/(\d{1,2})$/.exec(text.trim());
	if (!m) return null;
	const month = Number(m[1]);
	const day = Number(m[2]);
	// 2000 年はうるう年なので 2/29 も通る
	const d = new Date(2000, month - 1, day);
	if (d.getMonth() !== month - 1 || d.getDate() !== day) return null;
	return [month, day];
}

// ピッカーに渡す日付。年は使わないので、登録済みの年か今年で埋める（その年に無い 2/29 は 2000 年）。
const pickerValue = $derived.by(() => {
	const md = parseMonthDay(birthdayMd);
	if (!md) return "";
	const [month, day] = md;
	let year = birthdayYear ? Number(birthdayYear) : new Date().getFullYear();
	if (new Date(year, month - 1, day).getDate() !== day) year = 2000;
	return `${year}-${pad2(month)}-${pad2(day)}`;
});

// 形式違いは送信前にブラウザ標準の検証で止める。
$effect(() => {
	birthdayEl?.setCustomValidity(
		birthdayMd.trim() === "" || parseMonthDay(birthdayMd)
			? ""
			: "MM/DD の形で入力してください（例: 04/01）",
	);
});

// 誕生日欄のどこを押しても、重ねた日付ピッカーを開く（未対応ブラウザでは何もしない）。
function openPicker() {
	try {
		pickerEl?.showPicker?.();
	} catch {
		// ユーザー操作扱いにならない等で開けないときは、通常の入力欄のままにする
	}
}

// ピッカーで選んだ日付は月日だけ使う。
function pickBirthday(e: Event) {
	const value = (e.currentTarget as HTMLInputElement).value;
	const m = /^\d{4,}-(\d{2})-(\d{2})$/.exec(value);
	birthdayMd = m ? `${m[1]}/${m[2]}` : "";
}

function birthdayPayload(): string {
	const md = parseMonthDay(birthdayMd);
	if (!md) return "";
	return `${birthdayYear || "-"}-${pad2(md[0])}-${pad2(md[1])}`;
}

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
		birthday: birthdayPayload(),
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
				<div class="birthday-field">
					<input
						type="text"
						id="contact-birthday"
						class="picker-input"
						inputmode="numeric"
						autocomplete="off"
						placeholder="MM/DD"
						bind:this={birthdayEl}
						bind:value={birthdayMd}
						onclick={openPicker}
					/>
					<input
						type="date"
						class="birthday-picker"
						tabindex="-1"
						aria-hidden="true"
						value={pickerValue}
						bind:this={pickerEl}
						onchange={pickBirthday}
					/>
				</div>
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

<style>
	.birthday-field {
		position: relative;
	}
	.picker-input {
		cursor: pointer;
	}
	/* ピッカーを入力欄の位置に出すため、見えない日付欄を同じ場所に重ねる */
	.birthday-picker {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		margin: 0;
		padding: 0;
		border: 0;
		opacity: 0;
		pointer-events: none;
	}
</style>
