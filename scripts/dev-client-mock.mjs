import { spawn } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const clientDir = path.join(root, "client", "pwa");
const npm = process.platform === "win32" ? "npm.cmd" : "npm";
const children = [];

function start(label, args, env = {}) {
	const child = spawn(npm, args, {
		cwd: clientDir,
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

start("mock API", ["run", "mock"]);
start("Client", ["run", "dev:client"], {
	VITE_API_PROXY_TARGET: "http://localhost:8787",
});
