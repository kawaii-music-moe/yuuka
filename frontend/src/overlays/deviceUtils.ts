// デバイスフロー認可の純関数（旧 app.js formatDeviceCode）。DOM 非依存。

/** 入力を XXXX-XXXX 形式に整形（英数字のみ・大文字・8桁でハイフン挿入）。 */
export function formatDeviceCode(raw: string): string {
	const cleaned = String(raw || "")
		.toUpperCase()
		.replace(/[^A-Z0-9]/g, "")
		.slice(0, 8);
	return cleaned.length > 4
		? `${cleaned.slice(0, 4)}-${cleaned.slice(4)}`
		: cleaned;
}

/** コード発行からの経過秒を「N秒前 / N分前」に整形（承認前の端末確認表示用）。 */
export function formatElapsed(secs: number): string {
	const s = Math.max(0, Math.floor(secs));
	return s < 60 ? `${s}秒前` : `${Math.floor(s / 60)}分前`;
}

/** 端末名の表示の上限（文字数）。超えた分は「…」で省く。 */
export const DEVICE_NAME_DISPLAY_MAX = 60;

/**
 * アプリが申告した端末名を表示用に整える（サーバでも整えているが念のため）。
 * 制御文字・書字方向の制御文字・ゼロ幅文字を空白にし、連続する空白をまとめ、長すぎる名前は省く。
 */
export function displayDeviceName(
	raw: string | null | undefined,
	max = DEVICE_NAME_DISPLAY_MAX,
): string {
	const cleaned = String(raw ?? "")
		.replace(
			/[\p{Cc}\u{200B}-\u{200F}\u{202A}-\u{202E}\u{2066}-\u{2069}\u{FEFF}]/gu,
			" ",
		)
		.replace(/\s+/g, " ")
		.trim();
	if (!cleaned) return "（端末名なし）";
	const chars = Array.from(cleaned);
	return chars.length > max ? `${chars.slice(0, max).join("")}…` : cleaned;
}
