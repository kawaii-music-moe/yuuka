<script lang="ts">
// AI解析結果の報告モーダル（旧 index.html #modal-receipt-result）。
// Gemini のテキスト応答を表示（{@html} 不使用・文字列補間で自動エスケープ）。
import { Button, Modal } from "$lib/components/ui";

interface Props {
	open?: boolean;
	response?: string;
}

let { open = $bindable(false), response = "" }: Props = $props();
</script>

<Modal bind:open wide title="AI解析結果の報告">
	<div class="receipt-result-body">
		<div class="receipt-result-icon-container">
			<span class="material-symbols-outlined icon-display-large" aria-hidden="true"
				>analytics</span
			>
		</div>
		<h4>AI解析結果：</h4>
		<div class="receipt-ai-comment">{response}</div>
	</div>
	{#snippet footer()}
		<Button variant="primary" onclick={() => (open = false)}>確認しました</Button>
	{/snippet}
</Modal>
