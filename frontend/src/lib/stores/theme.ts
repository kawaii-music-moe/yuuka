import { writable } from "svelte/store";

// §9.3 / §12.2 テーマストア（localStorage + 副作用同期。applyTheme() の全副作用を再現）
export type Theme = "dark" | "light";

const KEY = "yuuka-theme";

// theme-init.js / manifest.json と整合する meta[theme-color] 色マップ
const THEME_COLORS: Record<Theme, string> = {
	dark: "#121212",
	light: "#FAFAFA",
};

function colorOf(t: Theme): string {
	return THEME_COLORS[t] ?? THEME_COLORS.dark;
}

function readInitial(): Theme {
	if (typeof window === "undefined") return "dark";
	// 廃止した blue-archive などの保存値はダークに戻す
	return localStorage.getItem(KEY) === "light" ? "light" : "dark";
}

export const theme = writable<Theme>(readInitial());

theme.subscribe((t) => {
	if (typeof document === "undefined") return;
	document.documentElement.setAttribute("data-theme", t);
	if (typeof localStorage !== "undefined") localStorage.setItem(KEY, t);
	document
		.querySelector('meta[name="theme-color"]')
		?.setAttribute("content", colorOf(t));
});

/** テーマを設定する。設定タブのボタンから呼ぶ。 */
export function setTheme(t: Theme): void {
	theme.set(t);
}

/** ヘッダ toggle 用: dark ↔ light を往復。 */
export function toggleTheme(): void {
	theme.update((t) => (t === "dark" ? "light" : "dark"));
}
