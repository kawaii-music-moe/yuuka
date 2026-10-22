import { spawn } from "node:child_process";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const api = process.env.VITE_API_TARGET ?? "http://127.0.0.1:3403";
const host = process.env.VITE_BIND_ADDR ?? "127.0.0.1";
const adminPort = process.env.VITE_PORT ?? "7855";
const clientPort = process.env.VITE_CLIENT_PORT ?? "5173";
const children = new Set();
let stopping = false;

function shutdown(code = 0) {
	if (stopping) return;
	stopping = true;
	process.exitCode = code;
	for (const child of children) child.kill("SIGTERM");
	const timeout = setTimeout(() => {
		for (const child of children) child.kill("SIGKILL");
	}, 5000);
	timeout.unref();
}

process.on("SIGINT", () => shutdown());
process.on("SIGTERM", () => shutdown());

function start(cwd, config, port, env) {
	const require = createRequire(path.join(cwd, "package.json"));
	const vite = path.join(
		path.dirname(require.resolve("vite/package.json")),
		"bin/vite.js",
	);
	const child = spawn(
		process.execPath,
		[vite, "--config", config, "--host", host, "--port", port, "--strictPort"],
		{
			cwd,
			stdio: "inherit",
			env: { ...process.env, ...env },
		},
	);
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

try {
	start(root, "frontend/vite.config.ts", adminPort, {
		VITE_API_TARGET: api,
		VITE_PWA_DEV_SERVER: `http://127.0.0.1:${clientPort}`,
	});
	start(path.join(root, "client/pwa"), "vite.config.ts", clientPort, {
		VITE_API_PROXY_TARGET: api,
		VITE_ADMIN_DEV_SERVER: `http://127.0.0.1:${adminPort}`,
	});
	console.log(
		`Dev: http://127.0.0.1:${clientPort}/ (admin: /admin/, API: ${api})`,
	);
} catch (error) {
	console.error(
		`${error.message}\nInstall dependencies: pnpm install --frozen-lockfile && npm --prefix client/pwa ci`,
	);
	shutdown(1);
}
