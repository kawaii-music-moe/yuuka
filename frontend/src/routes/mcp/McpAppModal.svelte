<script lang="ts">
// MCP Apps（io.modelcontextprotocol/ui）の画面を表示するモーダル。
//
// 1. /api/mcp-servers/:id/apps/resource でツールの画面（ui:// リソースの HTML と _meta.ui）を受け取る。
// 2. サンドボックスプロキシ（/mcp-apps/sandbox・MCP_SANDBOX_ORIGIN があればそのオリジン）を iframe で開く。
// 3. 公式 SDK の AppBridge でプロキシ越しに画面とつなぐ。プロキシの準備ができたら HTML を渡し、
//    画面の初期化が済んだら、ツールの引数（tool-input）と実行結果（tool-result）を送る。
// 4. 画面からの tools/call・resources/read はサーバー（apps/call・apps/resource）で代わりに実行する。
//
// 別オリジンが無いときは allow-same-origin を付けない（不透明オリジン）。本体の Cookie・DOM には
// どちらの場合も届かない。
import {
	AppBridge,
	PostMessageTransport,
} from "@modelcontextprotocol/ext-apps/app-bridge";
import { onDestroy, tick } from "svelte";
import { mcpApi } from "$lib/api/services";
import type { McpAppResourceResponse } from "$lib/api/types";
import { Button, Modal } from "$lib/components/ui";
import {
	clampFrameHeight,
	errorToolResult,
	isOpenableLink,
	needsArguments,
	parseArguments,
	sandboxAttribute,
} from "$lib/mcpApps";
import { theme } from "$lib/stores";

interface Props {
	open?: boolean;
	/** 対象（サーバーとツール）。requiresConfirmation はサーバーの「実行前に確認」設定。 */
	target: {
		serverId: number;
		serverName: string;
		tool: string;
		requiresConfirmation: boolean;
	} | null;
}

let { open = $bindable(false), target }: Props = $props();

type Phase = "idle" | "loading" | "confirm" | "running" | "error";
let phase = $state<Phase>("idle");
let errorMessage = $state("");
let resource = $state<McpAppResourceResponse | null>(null);
let argsText = $state("{}");
let argsError = $state("");
let frameHeight = $state(480);
let iframeEl = $state<HTMLIFrameElement | null>(null);

let bridge: AppBridge | null = null;
let startedFor: string | null = null;
let pendingArgs: Record<string, unknown> = {};

const title = $derived(
	target ? `${target.serverName} — ${target.tool}` : "MCP App",
);

function reset() {
	phase = "idle";
	errorMessage = "";
	resource = null;
	argsText = "{}";
	argsError = "";
	frameHeight = 480;
}

async function load(t: NonNullable<Props["target"]>) {
	phase = "loading";
	try {
		const res = await mcpApi.appsResource(t.serverId, t.tool);
		resource = res;
		if (t.requiresConfirmation || needsArguments(res.tool.inputSchema)) {
			phase = "confirm";
		} else {
			await run({});
		}
	} catch (e) {
		phase = "error";
		errorMessage = e instanceof Error ? e.message : String(e);
	}
}

async function confirmRun() {
	try {
		const args = parseArguments(argsText);
		argsError = "";
		await run(args);
	} catch (e) {
		argsError = e instanceof Error ? e.message : String(e);
	}
}

async function run(args: Record<string, unknown>) {
	pendingArgs = args;
	phase = "running";
	await tick();
	if (iframeEl?.contentWindow && resource && target) {
		await connect(iframeEl.contentWindow, resource, target.serverId);
	}
}

async function connect(
	win: Window,
	res: McpAppResourceResponse,
	serverId: number,
) {
	const ui = res.ui ?? {};
	const csp = ui.csp as Record<string, string[]> | undefined;
	const permissions = ui.permissions as Record<string, object> | undefined;
	const b = new AppBridge(
		null,
		{ name: "yuuka", version: "2.0.0" },
		{
			openLinks: {},
			serverTools: {},
			serverResources: {},
			logging: {},
			sandbox: { csp, permissions },
		},
		{
			hostContext: {
				toolInfo: {
					tool: {
						name: res.tool.name,
						description: res.tool.description ?? undefined,
						inputSchema: (res.tool.inputSchema ?? { type: "object" }) as {
							type: "object";
						},
						_meta: res.tool._meta,
					},
				},
				theme: $theme,
				displayMode: "inline",
				availableDisplayModes: ["inline"],
				containerDimensions: {
					maxHeight: Math.floor(window.innerHeight * 0.8),
				},
				locale: navigator.language || "ja-JP",
				timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone,
				platform: "web",
			},
		},
	);
	b.onsandboxready = () => {
		void b.sendSandboxResourceReady({ html: res.html, csp, permissions });
	};
	b.oninitialized = () => {
		void sendToolCall(b, serverId, res.tool.name);
	};
	b.oncalltool = async (params) => {
		const r = await mcpApi
			.appsCall(serverId, {
				name: params.name,
				arguments: params.arguments,
				from: "app",
			})
			.catch((e: unknown) => ({
				result: errorToolResult(e instanceof Error ? e.message : String(e)),
			}));
		return r.result as Awaited<
			ReturnType<NonNullable<AppBridge["oncalltool"]>>
		>;
	};
	b.onreadresource = async (params) => {
		const r = await mcpApi.appsRead(serverId, params.uri);
		return r.result as Awaited<
			ReturnType<NonNullable<AppBridge["onreadresource"]>>
		>;
	};
	b.onopenlink = async ({ url }) => {
		if (!isOpenableLink(url)) return { isError: true };
		window.open(url, "_blank", "noopener,noreferrer");
		return {};
	};
	// 管理画面には会話が無いので、画面からのメッセージ送信は受け付けない。
	b.onmessage = async () => ({ isError: true });
	b.onupdatemodelcontext = async () => ({});
	b.onrequestdisplaymode = async () => ({ mode: "inline" });
	b.onsizechange = ({ height }) => {
		if (typeof height === "number") {
			frameHeight = clampFrameHeight(height, window.innerHeight);
		}
	};
	b.onloggingmessage = (params) => {
		console.debug("[MCP App]", params);
	};
	bridge = b;
	await b.connect(new PostMessageTransport(win, win));
}

async function sendToolCall(b: AppBridge, serverId: number, tool: string) {
	await b.sendToolInput({ arguments: pendingArgs });
	try {
		const r = await mcpApi.appsCall(serverId, {
			name: tool,
			arguments: pendingArgs,
			from: "host",
		});
		await b.sendToolResult(
			r.result as Parameters<AppBridge["sendToolResult"]>[0],
		);
	} catch (e) {
		await b.sendToolResult(
			errorToolResult(e instanceof Error ? e.message : String(e)),
		);
	}
}

async function teardown() {
	const b = bridge;
	bridge = null;
	if (!b) return;
	// 画面に後片付けの機会を与える（応答しなくても 500ms で閉じる）。
	await Promise.race([
		b.teardownResource({}).catch(() => undefined),
		new Promise((resolve) => setTimeout(resolve, 500)),
	]);
	await b.close().catch(() => undefined);
}

// 開いたら読み込み、閉じたら片付ける。
$effect(() => {
	const key = open && target ? `${target.serverId}:${target.tool}` : null;
	if (key === startedFor) return;
	startedFor = key;
	void teardown().then(() => {
		reset();
		if (key && target) void load(target);
	});
});

onDestroy(() => {
	void teardown();
});

// テーマが変わったら画面に知らせる。
$effect(() => {
	const t = $theme;
	if (bridge) void bridge.sendHostContextChange({ theme: t });
});
</script>

<Modal bind:open {title} wide>
	<div class="mcp-app">
		{#if phase === "loading"}
			<p class="description-text">画面を読み込んでいます…</p>
		{:else if phase === "error"}
			<p class="mcp-app-error">{errorMessage}</p>
		{:else if phase === "confirm" && resource}
			<div class="mcp-app-confirm">
				<p>
					ツール <strong>{resource.tool.name}</strong> を実行して画面を開きます。
				</p>
				{#if resource.tool.description}
					<p class="description-text">{resource.tool.description}</p>
				{/if}
				<label for="mcp-app-args">引数（JSON）</label>
				<textarea id="mcp-app-args" class="int-mono" rows="6" bind:value={argsText}
				></textarea>
				{#if argsError}
					<p class="mcp-app-error">{argsError}</p>
				{/if}
				<Button variant="primary" onclick={confirmRun}>実行して開く</Button>
			</div>
		{/if}
		{#if phase === "running" && resource}
			<iframe
				bind:this={iframeEl}
				src={resource.sandbox.url}
				title={title}
				sandbox={sandboxAttribute(resource.sandbox.isolated)}
				class="mcp-app-frame"
				style:height={`${frameHeight}px`}
			></iframe>
		{/if}
	</div>
</Modal>

<style>
	.mcp-app {
		width: 100%;
		min-height: 200px;
	}
	.mcp-app-frame {
		width: 100%;
		border: 0;
		display: block;
		border-radius: 8px;
		background: transparent;
	}
	.mcp-app-confirm {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}
	.mcp-app-confirm textarea {
		width: 100%;
		box-sizing: border-box;
	}
	.mcp-app-error {
		color: var(--color-danger, #e5484d);
		white-space: pre-wrap;
	}
</style>
