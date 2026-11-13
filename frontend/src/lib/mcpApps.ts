// MCP Apps（io.modelcontextprotocol/ui）のホスト側で使う小さな純関数。
// 画面とのやり取り（ui/initialize・tool-input・tool-result 等）は公式 SDK の AppBridge が担う
// （routes/mcp/McpAppModal.svelte）。ここには表示の判断だけを置く。

/** 不透明オリジン（MCP_SANDBOX_ORIGIN 未設定）と別オリジンで、サンドボックス iframe の属性を変える。 */
export function sandboxAttribute(isolated: boolean): string {
	// 別オリジンなら allow-same-origin を付けて、画面に実オリジン（localStorage 等）を持たせる。
	// 本体と同じオリジンでは allow-same-origin を決して付けない（付けると隔離が無効になる）。
	return isolated
		? "allow-scripts allow-same-origin allow-forms"
		: "allow-scripts allow-forms";
}

/** inputSchema に必須の引数があるか（あれば開く前に引数を入力してもらう）。 */
export function needsArguments(inputSchema: unknown): boolean {
	if (!inputSchema || typeof inputSchema !== "object") return false;
	const required = (inputSchema as { required?: unknown }).required;
	return Array.isArray(required) && required.length > 0;
}

/** 引数入力欄（JSON）を読み取る。オブジェクト以外はエラー。 */
export function parseArguments(text: string): Record<string, unknown> {
	const trimmed = text.trim();
	if (trimmed === "") return {};
	const value: unknown = JSON.parse(trimmed);
	if (!value || typeof value !== "object" || Array.isArray(value)) {
		throw new Error("引数は JSON オブジェクトで入力してください。");
	}
	return value as Record<string, unknown>;
}

/** 画面から要求された高さを、表示できる範囲に収める。 */
export function clampFrameHeight(requested: number, viewport: number): number {
	const max = Math.max(240, Math.floor(viewport * 0.8));
	if (!Number.isFinite(requested)) return Math.min(480, max);
	return Math.min(Math.max(Math.ceil(requested), 120), max);
}

/** 画面からのリンクを開いてよいか（http/https だけ）。 */
export function isOpenableLink(url: string): boolean {
	try {
		const u = new URL(url);
		return u.protocol === "https:" || u.protocol === "http:";
	} catch {
		return false;
	}
}

/** ツールのエラーを、画面へ渡す CallToolResult の形にする。 */
export function errorToolResult(message: string): {
	content: { type: "text"; text: string }[];
	isError: true;
} {
	return { content: [{ type: "text", text: message }], isError: true };
}
