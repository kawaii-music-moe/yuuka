<script lang="ts">
// ペルソナ追加モーダル。「マーケットから追加」「自分で作成」を選び、
// マーケットを選んだ場合は公開ペルソナ一覧（全文表示・インポート）に切り替える。

import type { PublicPersonaView } from "$lib/api/types";
import { Button, EmptyState, Icon, Modal } from "$lib/components/ui";

interface Props {
	open?: boolean;
	marketplace: PublicPersonaView[];
	/** 全文プレビューなど、上に別モーダルを重ねている間は ESC で閉じない。 */
	closeOnEsc?: boolean;
	oncreate: () => void;
	onpreview: (m: PublicPersonaView) => void;
	onimport: (id: number) => void;
}

let {
	open = $bindable(false),
	marketplace,
	closeOnEsc = true,
	oncreate,
	onpreview,
	onimport,
}: Props = $props();

let step = $state<"choose" | "market">("choose");

$effect(() => {
	if (open) step = "choose";
});
</script>

<Modal
	bind:open
	wide
	{closeOnEsc}
	title={step === "market" ? "マーケットから追加" : "ペルソナを追加"}
>
	{#if step === "choose"}
		<div class="persona-add-choices">
			<button type="button" class="persona-add-choice" onclick={oncreate}>
				<Icon name="edit_note" size={28} />
				<span class="persona-add-choice-title">自分で作成</span>
				<span class="persona-add-choice-desc">名前と定義を入力して新しく作ります。</span>
			</button>
			<button
				type="button"
				class="persona-add-choice"
				onclick={() => (step = "market")}
			>
				<Icon name="storefront" size={28} />
				<span class="persona-add-choice-title">マーケットから追加</span>
				<span class="persona-add-choice-desc">公開されているペルソナを取り込みます。</span>
			</button>
		</div>
	{:else}
		<div class="list-container persona-add-market">
			{#if marketplace.length > 0}
				{#each marketplace as m (m.id)}
					<div class="card-item glass persona-row persona-add-item">
						<div class="persona-row-head">
							<span class="entry-name">{m.name}</span>
							<span class="persona-row-meta">
								by {m.owner_username} ・ {m.prompt.length.toLocaleString()} 文字
							</span>
						</div>
						<div class="persona-row-preview">{m.prompt.slice(0, 120)}</div>
						<div class="persona-row-actions">
							<button type="button" class="btn-mini" onclick={() => onpreview(m)}>
								<Icon name="visibility" />全文表示
							</button>
							<button type="button" class="btn-mini" onclick={() => onimport(m.id)}>
								<Icon name="download" />インポート
							</button>
						</div>
					</div>
				{/each}
			{:else}
				<EmptyState
					icon="storefront"
					message="公開されているペルソナはまだありません。"
				/>
			{/if}
		</div>
	{/if}
	{#snippet footer()}
		{#if step === "market"}
			<Button variant="secondary" onclick={() => (step = "choose")}>戻る</Button>
		{/if}
	{/snippet}
</Modal>

<style>
	.persona-add-choices {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: 12px;
	}
	.persona-add-choice {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 6px;
		padding: 24px 16px;
		font: inherit;
		color: var(--text-primary);
		text-align: center;
		cursor: pointer;
		background: transparent;
		border: 1px solid var(--border-matte);
		border-radius: var(--radius);
		transition: var(--transition-smooth);
	}
	.persona-add-choice:hover {
		border-color: var(--border-focus);
		background-color: rgba(255, 255, 255, 0.04);
	}
	.persona-add-choice-title {
		font-weight: 700;
	}
	.persona-add-choice-desc {
		font-size: 0.78rem;
		color: var(--color-zinc-muted);
	}
	.persona-add-market {
		max-height: 60vh;
		overflow-x: hidden;
		overflow-y: auto;
	}
	/* 共通の .card-item の左右 -12px をやめ、モーダルの幅に収める */
	.persona-add-item {
		margin: 0;
	}
	@media (max-width: 480px) {
		.persona-add-choices {
			grid-template-columns: 1fr;
		}
	}
</style>
