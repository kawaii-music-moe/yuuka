import path from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

export default defineConfig({
	// vite.config.ts と同じく明示する: root を frontend/ に固定しないと、リポジトリ直下
	// （legacy Node 版）の src/**/*.test.ts を拾ってしまう（include はこの root からの相対）。
	root: __dirname,
	// #34: router.ts の BASE_PATH は `import.meta.env.BASE_URL` から導出される
	// （frontend/vite.config.ts の `base: "/admin/"` と一致させる設計）。テストでも
	// 同じ base を与えないと BASE_PATH が空文字列になり、/admin プレフィックスの
	// 検証にならないため、ここで明示的に一致させる。
	base: "/admin/",
	test: {
		// Node 環境（DOM 不要）。router.ts の純粋関数のみを対象とする。
		environment: "node",
		include: ["src/**/*.test.ts"],
	},
});
