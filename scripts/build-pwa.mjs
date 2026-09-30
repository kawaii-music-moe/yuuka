import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { cp, rm } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

// client/pwa is managed with npm (package-lock.json) and lives outside the pnpm
// workspace (pnpm-workspace.yaml only lists '.'), so `pnpm install` at the repo
// root never installs its dependencies. Install them here on demand so that a
// clean checkout can run `pnpm build:pwa` without a separate manual step
// (see issue #45).
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const clientDir = path.join(root, "client", "pwa");
// 出力先は Rust 側の配信元（crates/yuuka-supervisor の PWA_DIST_DIR）と一致させる。
// 管理画面 SPA のビルド（`pnpm build:front`・`emptyOutDir: true`）は dist/public 全体を
// 空にするため、必ず build:front の後に実行すること（root の `pnpm build` はこの順で実行する）。
const target = path.join(root, "dist", "public", "pwa");

if (!existsSync(path.join(clientDir, "node_modules"))) {
	const install = spawnSync("npm ci", {
		cwd: clientDir,
		stdio: "inherit",
		shell: true,
	});
	if (install.error) throw install.error;
	if (install.status !== 0) process.exit(install.status ?? 1);
}

const run = spawnSync("npm run build", {
	cwd: clientDir,
	stdio: "inherit",
	shell: true,
});
if (run.error) throw run.error;
if (run.status !== 0) process.exit(run.status ?? 1);

await rm(target, { recursive: true, force: true });
await cp(path.join(clientDir, "dist"), target, { recursive: true });
