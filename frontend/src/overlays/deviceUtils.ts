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
