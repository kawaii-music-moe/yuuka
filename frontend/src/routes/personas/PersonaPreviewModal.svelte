<script lang="ts">
// マーケットプレイス ペルソナ全文プレビュー（旧 index.html #modal-persona-preview）。
// 「このペルソナをインポート」で親の onimport を呼ぶ。
import { Button, Modal } from "$lib/components/ui";

interface Props {
	open?: boolean;
	personaName?: string;
	prompt?: string;
	/** インポート対象の marketplace persona id。 */
	personaId?: number | null;
	onimport: (id: number) => void;
}

let {
	open = $bindable(false),
	personaName = "",
	prompt = "",
	personaId = null,
	onimport,
}: Props = $props();
</script>

<Modal
	bind:open
	wide
	title={personaName ? `ペルソナ プレビュー: ${personaName}` : "ペルソナ プレビュー"}
>
	<pre class="persona-preview-prompt">{prompt}</pre>
	{#snippet footer()}
		<Button
			variant="primary"
			onclick={() => {
				if (personaId != null) onimport(personaId);
			}}>このペルソナをインポート</Button
		>
	{/snippet}
</Modal>

<style>
	.persona-preview-prompt {
		white-space: pre-wrap;
		word-break: break-word;
		max-height: 420px;
		overflow-y: auto;
		font-family: inherit;
		font-size: 0.88rem;
		line-height: 1.6;
		background: var(--surface-2dp);
		border: 1px solid var(--border-matte);
		border-radius: var(--radius);
		padding: 14px;
		margin: 0 0 16px;
	}
</style>
