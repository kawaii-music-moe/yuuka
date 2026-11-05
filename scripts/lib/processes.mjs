// dev 用スクリプト（dev.mjs・dev-client-mock.mjs）で共通の子プロセス管理。
import { spawn } from "node:child_process";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const root = path.resolve(
	path.dirname(fileURLToPath(import.meta.url)),
	"../..",
);
export const clientDir = path.join(root, "client", "pwa");
export const installHint =
	"Install dependencies: pnpm install --frozen-lockfile && npm --prefix client/pwa ci";

/** `cwd` の node_modules にある vite の CLI（見つからなければ例外）。 */
export function viteBin(cwd) {
	const require = createRequire(path.join(cwd, "package.json"));
	return path.join(
		path.dirname(require.resolve("vite/package.json")),
		"bin/vite.js",
	);
}

/**
 * 子プロセスをまとめて動かす。どれか 1 つが終わるか SIGINT/SIGTERM を受けたら全部を止める
 * （SIGTERM のあと 5 秒で SIGKILL）。終了コードは最初に終わった子のもの（シグナル停止は 0）。
 */
export function processGroup() {
	const children = new Set();
	let stopping = false;

	function shutdown(code = 0) {
		if (stopping) return;
		stopping = true;
		process.exitCode = code;
		for (const child of children) child.kill("SIGTERM");
		setTimeout(() => {
			for (const child of children) child.kill("SIGKILL");
		}, 5000).unref();
	}

	process.on("SIGINT", () => shutdown());
	process.on("SIGTERM", () => shutdown());

	function start(command, args, { cwd = root, env = {}, shell = false } = {}) {
		const child = spawn(command, args, {
			cwd,
			stdio: "inherit",
			env: { ...process.env, ...env },
			shell,
		});
		children.add(child);
		child.on("error", (error) => {
			console.error(error.message);
			shutdown(1);
		});
		child.on("exit", (code) => {
			children.delete(child);
			if (!stopping) shutdown(code || 1);
		});
	}

	/** `cwd` の vite を `args` 付きで起動する。 */
	function vite(cwd, args, env) {
		start(process.execPath, [viteBin(cwd), ...args], { cwd, env });
	}

	return { start, vite, shutdown };
}
