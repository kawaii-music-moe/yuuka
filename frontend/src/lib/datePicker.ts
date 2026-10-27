// 日付・時刻の入力欄（date / time / datetime-local / month / week）を、文字では打たせず
// 欄のどこを押してもブラウザ標準のピッカーを開くようにする。
// document に 1 度だけ登録し、画面ごとの onclick は不要にする（main.ts で呼ぶ）。

const PICKER_TYPES = new Set([
	"date",
	"time",
	"datetime-local",
	"month",
	"week",
]);

export type PickerKeyAction = "pass" | "open" | "clear" | "block";

/**
 * 日付欄でのキー操作の扱い。
 * - Tab（移動）・Enter（フォーム送信）・修飾キー付き（コピー等）はそのまま通す
 * - Space / ↑ / ↓ はピッカーを開く
 * - Backspace / Delete は値を空にする（任意の日付を消せるように）
 * - それ以外（数字の直接入力など）は止める
 */
export function pickerKeyAction(
	key: string,
	mods: { ctrlKey?: boolean; metaKey?: boolean; altKey?: boolean } = {},
): PickerKeyAction {
	if (mods.ctrlKey || mods.metaKey || mods.altKey) return "pass";
	if (key === "Tab" || key === "Enter" || key === "Escape") return "pass";
	if (key === " " || key === "ArrowDown" || key === "ArrowUp") return "open";
	if (key === "Backspace" || key === "Delete") return "clear";
	return "block";
}

function pickerInput(target: EventTarget | null): HTMLInputElement | null {
	return target instanceof HTMLInputElement &&
		PICKER_TYPES.has(target.type) &&
		!target.disabled &&
		!target.readOnly
		? target
		: null;
}

/** ピッカーを開く。未対応ブラウザや、ユーザー操作扱いにならないときは何もしない。 */
export function openPicker(input: HTMLInputElement): void {
	try {
		input.showPicker?.();
	} catch {
		// 開けないときは通常の入力欄のままにする
	}
}

export function installDatePickers(root: Document = document): void {
	root.addEventListener("click", (e) => {
		const input = pickerInput(e.target);
		if (input) openPicker(input);
	});
	root.addEventListener("keydown", (e) => {
		const input = pickerInput(e.target);
		if (!input) return;
		const action = pickerKeyAction(e.key, e);
		if (action === "pass") return;
		e.preventDefault();
		if (action === "open") {
			openPicker(input);
		} else if (action === "clear" && input.value !== "") {
			input.value = "";
			// bind:value / onchange に届くよう、手で入力したときと同じイベントを出す
			input.dispatchEvent(new Event("input", { bubbles: true }));
			input.dispatchEvent(new Event("change", { bubbles: true }));
		}
	});
	// 貼り付け・ドロップでの文字入力も受け付けない
	for (const type of ["paste", "drop"] as const) {
		root.addEventListener(type, (e) => {
			if (pickerInput(e.target)) e.preventDefault();
		});
	}
}
