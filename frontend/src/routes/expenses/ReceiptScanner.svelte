<script lang="ts">
// AIレシートスキャナー（旧 index.html #receipt-dropzone/#receipt-file-input/#scan-status
// + app.js:4374 handleReceiptScan のドロップゾーン部）。
// ファイル選択は onpick で親へ委譲（親が base64 化 → financeApi.uploadReceipt）。
import { pushToast } from "$lib/stores/toast";

interface Props {
	/** 解析中フラグ（親が制御。スピナー表示）。 */
	scanning?: boolean;
	/** 画像ファイルが選択されたとき（親が解析する）。 */
	onpick: (file: File) => void;
}

let { scanning = false, onpick }: Props = $props();

let dragover = $state(false);
let fileInput: HTMLInputElement | undefined = $state();

function pick(file: File | undefined) {
	if (!file) return;
	// 旧 handleReceiptScan の画像 MIME ガード（alert → pushToast）。
	if (!file.type.startsWith("image/")) {
		pushToast(
			"レシート解析は画像ファイル（PNG/JPEG）のみ対応しています。",
			"error",
		);
		return;
	}
	onpick(file);
}

function onDrop(e: DragEvent) {
	e.preventDefault();
	dragover = false;
	pick(e.dataTransfer?.files?.[0]);
}

function onChange(e: Event) {
	const input = e.currentTarget as HTMLInputElement;
	pick(input.files?.[0]);
	input.value = ""; // 同じファイルの再選択を許可
}
</script>

<div class="action-column card">
	<div class="column-header">
		<h3>
			<span class="material-symbols-outlined header-icon-symbol" aria-hidden="true"
				>document_scanner</span
			>AIレシートスキャナー
		</h3>
		<span class="badge badge-accent">Gemini連携</span>
	</div>
	<p class="description-text">
		レシート画像をドラッグ＆ドロップするか、ファイルを選択してください。AIが自動的に分類・登録を行います。
	</p>

	<div
		class="dropzone"
		class:dragover
		role="button"
		tabindex="0"
		onclick={() => fileInput?.click()}
		onkeydown={(e) => {
			if (e.key === "Enter" || e.key === " ") {
				e.preventDefault();
				fileInput?.click();
			}
		}}
		ondragover={(e) => {
			e.preventDefault();
			dragover = true;
		}}
		ondragleave={() => (dragover = false)}
		ondrop={onDrop}
	>
		<div class="dropzone-content">
			<span class="material-symbols-outlined dropzone-icon-symbol" aria-hidden="true"
				>cloud_upload</span
			>
			<p>画像をドロップ、または<span class="text-highlight">ブラウズ</span></p>
			<span class="dropzone-sub">PNG, JPEG / 最大 5MB</span>
			<input
				type="file"
				accept="image/*"
				class="hidden"
				bind:this={fileInput}
				onchange={onChange}
			/>
		</div>
	</div>

	{#if scanning}
		<div class="scan-status-box">
			<div class="spinner"></div>
			<p>レシート画像をアップロードしてAIに渡しています...</p>
		</div>
	{/if}
</div>
