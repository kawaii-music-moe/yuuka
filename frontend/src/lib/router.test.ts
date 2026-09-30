// ─────────────────────────────────────────────────────────────────────────────
// router.ts の純粋関数の単体テスト（#34）。
//
// cleanPath/isPublicPath/isAllowedReturnPath は BASE_PATH（import.meta.env.BASE_URL）に
// 依存しないため、通常の静的 import で検証する。
//
// withBasePath/resolveRoute は BASE_PATH（"/admin"）に依存するが、Vitest は（vite build/dev と
// 異なり）frontend/vite.config.ts の `base` を `import.meta.env.BASE_URL` へ自動反映しない
// （実測: 素の import では常に "/" になる）。そのため該当 describe だけ `vi.stubEnv` で
// `BASE_URL` を本番と同じ "/admin/" に固定し、`vi.resetModules()` でモジュールキャッシュを
// 破棄してから router.ts を再度 import して BASE_PATH を再評価させる。
// ─────────────────────────────────────────────────────────────────────────────

import { beforeAll, describe, expect, it, vi } from "vitest";
import { cleanPath, isAllowedReturnPath, isPublicPath } from "./router";

describe("cleanPath", () => {
	it("strips query and hash", () => {
		expect(cleanPath("/bot/dashboard?foo=1#frag")).toBe("/bot/dashboard");
	});

	it("strips trailing slash", () => {
		expect(cleanPath("/bot/dashboard/")).toBe("/bot/dashboard");
	});

	it("collapses empty path to /", () => {
		expect(cleanPath("")).toBe("/");
		expect(cleanPath("/")).toBe("/");
	});

	it("never strips BASE_PATH (pure function, no /admin awareness)", () => {
		// cleanPath はアプリ相対パスの正規化のみを行う。物理 URL（/admin プレフィックス付き）を
		// 渡した場合でも BASE_PATH を剥がさない（それは stripBasePath/resolveRoute の責務）。
		expect(cleanPath("/admin")).toBe("/admin");
		expect(cleanPath("/admin/bot/dashboard")).toBe("/admin/bot/dashboard");
	});
});

describe("isPublicPath", () => {
	it("matches §8 PUBLIC_PATHS", () => {
		expect(isPublicPath("/usage")).toBe(true);
		expect(isPublicPath("/terms")).toBe(true);
		expect(isPublicPath("/privacy")).toBe(true);
		expect(isPublicPath("/tasks/guide")).toBe(true);
	});

	it("rejects everything else", () => {
		expect(isPublicPath("/")).toBe(false);
		expect(isPublicPath("/bot/dashboard")).toBe(false);
		expect(isPublicPath("/login")).toBe(false);
		expect(isPublicPath("/admin")).toBe(false);
	});
});

describe("isAllowedReturnPath (open-redirect 対策・#34 症状4)", () => {
	it("allows same-origin absolute app paths", () => {
		expect(isAllowedReturnPath("/bot/dashboard")).toBe(true);
		expect(isAllowedReturnPath("/")).toBe(true);
		expect(isAllowedReturnPath("/admin")).toBe(true);
		expect(isAllowedReturnPath("/bot/dashboard?tab=x&y=1")).toBe(true);
		expect(isAllowedReturnPath("/device?code=WDJB-MJHT")).toBe(true);
	});

	it("rejects empty/missing input", () => {
		expect(isAllowedReturnPath(null)).toBe(false);
		expect(isAllowedReturnPath(undefined)).toBe(false);
		expect(isAllowedReturnPath("")).toBe(false);
	});

	it("rejects paths that don't start with a single /", () => {
		expect(isAllowedReturnPath("bot/dashboard")).toBe(false);
		expect(isAllowedReturnPath("relative/path")).toBe(false);
	});

	it("rejects scheme-relative URLs (host swap)", () => {
		expect(isAllowedReturnPath("//evil.example")).toBe(false);
		expect(isAllowedReturnPath("///evil.example")).toBe(false);
		expect(isAllowedReturnPath("//evil.example/x")).toBe(false);
	});

	it("rejects absolute URLs with a scheme", () => {
		expect(isAllowedReturnPath("http://evil.example")).toBe(false);
		expect(isAllowedReturnPath("https://evil.example/admin")).toBe(false);
		expect(isAllowedReturnPath("javascript:alert(1)")).toBe(false);
	});

	it("rejects backslash tricks that browsers may treat as scheme-relative", () => {
		expect(isAllowedReturnPath("/\\evil.example")).toBe(false);
		expect(isAllowedReturnPath("\\\\evil.example")).toBe(false);
	});

	it("rejects control-character smuggling that URL parsing would turn scheme-relative", () => {
		// WHATWG URL はタブ/改行を除去してから解析するため、"/\t/evil.example" は
		// 一見 "//" で始まっていなくても実質 "//evil.example" に化ける。
		expect(isAllowedReturnPath("/\t/evil.example")).toBe(false);
		expect(isAllowedReturnPath("/\n/evil.example")).toBe(false);
	});

	it("allows percent-encoded slashes in the path segment (still same-origin)", () => {
		// %2F はブラウザ側でパスセパレータとして展開されないため、オープンリダイレクトにならない。
		expect(isAllowedReturnPath("/%2F%2Fevil.example")).toBe(true);
	});
});

// ── BASE_PATH（"/admin"）依存の関数 ─────────────────────────────────────────
describe("with BASE_PATH=/admin (frontend/vite.config.ts の base と一致)", () => {
	let withBasePath: (path: string) => string;
	let resolveRoute: (input: URL | string) => {
		view: string;
		tab?: string;
		params?: Record<string, string>;
	};

	beforeAll(async () => {
		vi.resetModules();
		vi.stubEnv("BASE_URL", "/admin/");
		// 上の describe 群が既に読み込んだ（BASE_URL="/" 時点の）router.ts とは別インスタンスとして
		// 再評価させる（resetModules によりキャッシュが破棄され、この import で BASE_PATH が
		// "/admin" として再計算される）。
		const mod = await import("./router");
		withBasePath = mod.withBasePath;
		resolveRoute = mod.resolveRoute;
	});

	describe("withBasePath", () => {
		it("prefixes app-relative paths with BASE_PATH", () => {
			expect(withBasePath("/bot/dashboard")).toBe("/admin/bot/dashboard");
			expect(withBasePath("/login")).toBe("/admin/login");
		});

		it("maps the app root to BASE_PATH + trailing slash", () => {
			expect(withBasePath("/")).toBe("/admin/");
		});

		it("maps the in-app admin route to BASE_PATH + /admin (no collision)", () => {
			// アプリ相対パス "/admin"（AdminOverlay）と配信プレフィックス "/admin" は別物。
			// withBasePath は物理 URL "/admin/admin" を返す。
			expect(withBasePath("/admin")).toBe("/admin/admin");
		});

		it("is idempotent when already base-prefixed", () => {
			expect(withBasePath("/admin/bot/dashboard")).toBe("/admin/bot/dashboard");
			expect(withBasePath("/admin/")).toBe("/admin/");
		});
	});

	describe("resolveRoute (物理 URL 入力を前提とする)", () => {
		const toUrl = (path: string) => new URL(path, "http://localhost");

		it("resolves the physical SPA root to the bots view", () => {
			expect(resolveRoute(toUrl("/admin")).view).toBe("bots");
			expect(resolveRoute(toUrl("/admin/")).view).toBe("bots");
		});

		it("resolves the physical /admin/admin path to the admin view (no collision with BASE_PATH)", () => {
			expect(resolveRoute(toUrl("/admin/admin")).view).toBe("admin");
		});

		it("resolves public pages under the admin prefix", () => {
			expect(resolveRoute(toUrl("/admin/usage")).view).toBe("usage");
			expect(resolveRoute(toUrl("/admin/terms")).view).toBe("terms");
			expect(resolveRoute(toUrl("/admin/privacy")).view).toBe("privacy");
			expect(resolveRoute(toUrl("/admin/tasks/guide")).view).toBe(
				"tasks-guide",
			);
		});

		it("resolves /device and keeps the ?code= param", () => {
			const resolved = resolveRoute(toUrl("/admin/device?code=WDJB-MJHT"));
			expect(resolved.view).toBe("device");
			expect(resolved.params).toEqual({ code: "WDJB-MJHT" });
		});

		it("resolves bot tabs and falls back to the default tab for unknown ones", () => {
			expect(resolveRoute(toUrl("/admin/bot/tasks")).tab).toBe("tasks");
			expect(resolveRoute(toUrl("/admin/bot/does-not-exist")).tab).toBe(
				"config",
			);
			expect(resolveRoute(toUrl("/admin/bot")).tab).toBe("config");
		});

		it("falls back to notfound for unknown paths under the admin prefix", () => {
			expect(resolveRoute(toUrl("/admin/nope")).view).toBe("notfound");
		});
	});
});
