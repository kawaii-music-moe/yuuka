// 読み上げの準備状況（どこで声を作るか）と、その表示。DOM 非依存。

export type EngineStatus =
	| { kind: "checking" }
	| { kind: "needs-download"; totalBytes: number }
	| { kind: "loading"; loaded: number; total: number }
	| { kind: "ready"; adapter: string }
	/** サーバーで Irodori-TTS を動かす（この端末の GPU で作れない・設定でサーバーを選んだ）。 */
	| { kind: "server"; reason: string }
	/** `retry` は読み込みの失敗（端末が非対応なのではない）で、やり直せる。 */
	| { kind: "browser"; reason: string; retry?: boolean }
	| { kind: "none"; reason: string };

/** Irodori-TTS の声で読める（この端末かサーバーで）。 */
export const usesIrodori = (status: EngineStatus) =>
	status.kind === "ready" || status.kind === "server";

/** バイト数を GB（小数 1 桁）で表す。 */
export const gb = (bytes: number) => (bytes / 2 ** 30).toFixed(1);

/** 音声モデルの読み込みの進み具合（0〜100）。読み込み中でなければ 0。 */
export function loadingPercent(status: EngineStatus): number {
	return status.kind === "loading"
		? Math.min(100, (status.loaded / Math.max(status.total, 1)) * 100)
		: 0;
}

/** 準備状況の案内文。この端末で準備ができていれば空。 */
export function describeEngine(status: EngineStatus): string {
	switch (status.kind) {
		case "checking":
			return "読み上げの準備を確認しています。";
		case "needs-download":
			return `この端末で声を作るには、音声モデル（約 ${gb(status.totalBytes)} GB）を一度ダウンロードします。準備ができるまでは標準の読み上げを使います。`;
		case "loading":
			return `音声モデルを準備しています（${Math.floor((status.loaded / Math.max(status.total, 1)) * 100)}%）。準備ができるまでは標準の読み上げを使います。`;
		case "ready":
			return "";
		case "server":
			return `サーバーで声を作ります（${status.reason}）。しばらく使わなかったあとの最初の 1 回は、準備に 10〜20 秒かかります。`;
		case "browser":
			return `ブラウザ標準の読み上げを使います（${status.reason}）。`;
		case "none":
			return `読み上げは使えません。返答は画面に表示します（${status.reason}）。`;
	}
}

/**
 * マイクの下に出す「今の声」。`voiceName` は選んでいる声の名前、`serverAvailable` はこの端末の
 * モデルを準備している間にサーバーで作れるか。
 */
export function describeCurrentVoice(
	status: EngineStatus,
	voiceName: string,
	serverAvailable: boolean,
): string {
	switch (status.kind) {
		case "browser":
			return "ブラウザ標準の読み上げ";
		case "none":
			return "読み上げなし（返答は画面に表示します）";
		case "ready":
			return `声: ${voiceName}`;
		case "server":
			return `声: ${voiceName}（サーバーで生成）`;
		default:
			return `声: ${voiceName}（準備ができるまでは${serverAvailable ? "サーバーで生成" : "標準の読み上げ"}）`;
	}
}
