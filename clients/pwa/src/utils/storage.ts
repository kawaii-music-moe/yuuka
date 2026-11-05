// この端末（ブラウザ）だけに残す設定の保存。プライベートブラウズなどで localStorage が使えなくても
// 例外にはせず、保存しないまま（このタブの中だけで）動き続ける。

/** 保存した値。無い・読めないときは null。 */
export function readStorage(key: string): string | null {
	try {
		return window.localStorage.getItem(key);
	} catch {
		return null;
	}
}

export function writeStorage(key: string, value: string): void {
	try {
		window.localStorage.setItem(key, value);
	} catch {
		// 保存できなくても、このタブの中では切り替わったまま動く。
	}
}

export function removeStorage(key: string): void {
	try {
		window.localStorage.removeItem(key);
	} catch {
		// 消せなくても、このタブの中では切り替わったまま動く。
	}
}
