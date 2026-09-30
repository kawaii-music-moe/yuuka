<script lang="ts">
// MCP サーバー管理ページ モーダル（旧 openMcpDashboard / teardownMcpDashboard）。
//
// dashboard は /api/mcp-servers/:id/dashboard が独自 CSP で text/html を返す real URL。
// sandbox="allow-scripts allow-forms"（allow-same-origin 無し）で不透明オリジンに隔離される。
// iframe は bind:this で掴み、閉じるとき（onDestroy / open=false）に src を空にして teardown する
// （replaceChildren 相当。ロード継続やメモリ保持を防ぐ）。
//
// ※ dev（Vite 前段）で iframe が期待どおり動くかは未検証（P4 で iframe 単独検証要）。
import { onDestroy } from "svelte";
import { mcpApi } from "$lib/api/services";
import { Modal } from "$lib/components/ui";

interface Props {
	open?: boolean;
	/** 対象サーバ（id と name） */
	server: { id: number; name: string } | null;
}

let { open = $bindable(false), server }: Props = $props();

let iframeEl = $state<HTMLIFrameElement | null>(null);

const title = $derived(
	server ? `${server.name} の管理ページ` : "MCPサーバー管理ページ",
);
const src = $derived(open && server ? mcpApi.dashboardUrl(server.id) : "");

// 閉じたら iframe を空にして teardown（ロード継続・不要な接続を止める）。
$effect(() => {
	if (!open && iframeEl) {
		iframeEl.src = "about:blank";
	}
});

onDestroy(() => {
	if (iframeEl) iframeEl.src = "about:blank";
});
</script>

<Modal bind:open {title} wide>
	<div class="mcp-dash-container">
		{#if open && server}
			<iframe
				bind:this={iframeEl}
				{src}
				title={title}
				sandbox="allow-scripts allow-forms"
				class="mcp-dash-frame"
			></iframe>
		{/if}
	</div>
</Modal>

<style>
	.mcp-dash-container {
		width: 100%;
		min-height: 70vh;
		border-radius: 8px;
		background: #fff;
		overflow: auto;
	}
	.mcp-dash-frame {
		width: 100%;
		height: 75vh;
		border: 0;
		display: block;
		background: #fff;
	}
</style>
