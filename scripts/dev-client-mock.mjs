import { spawn } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const clientDir = path.join(root, "client", "pwa");
const npm = process.platform === "win32" ? "npm.cmd" : "npm";
const pnpm = process.platform === "win32" ? "pnpm.cmd" : "pnpm";
const children = [];

function start(label, command, args, env = {}, cwd = clientDir) {
	const child = spawn(command, args, {
		cwd,
		stdio: "inherit",
		env: { ...process.env, ...env },
		shell: process.platform === "win32",
	});
	child.on("exit", (code) => {
		if (code && code !== 0) process.exitCode = code;
	});
	children.push(child);
	console.log(`[dev] ${label} started`);
}

function shutdown() {
	for (const child of children) child.kill();
}

process.on("SIGINT", shutdown);
process.on("SIGTERM", shutdown);

start("mock API", npm, ["run", "mock"]);
start("Client", npm, ["run", "dev:client"], {
	VITE_API_PROXY_TARGET: "http://localhost:8787",
});
// 共有ログイン/管理画面（frontend の Svelte dev server）。Client の /login・/admin は
// ここへリダイレクトされる（client/pwa/vite.config.ts 参照）。Client(5173) と衝突しない
// ポートに固定し、/api はこの mock API へ向ける（#44: 撤去済み src/public を見て 500 に
// なっていたのを修正）。
start(
	"Admin",
	pnpm,
	["run", "dev:front", "--", "--port", "5174", "--strictPort"],
	{ VITE_API_TARGET: "http://localhost:8787" },
	root,
);
