import { spawn } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const pnpm = process.platform === "win32" ? "pnpm.cmd" : "pnpm";
const children = [];

function start(label, args, env = {}) {
	const child = spawn(pnpm, args, {
		cwd: root,
		stdio: "inherit",
		env: { ...process.env, ...env },
		shell: process.platform === "win32",
	});
	children.push(child);
	console.log(`[dev] ${label} started`);
}

function shutdown() {
	for (const child of children) child.kill();
}

process.on("SIGINT", shutdown);
process.on("SIGTERM", shutdown);

start("Client and mock API", ["dev:client:mock"]);
start("Admin", ["dev:front"], { VITE_API_TARGET: "http://localhost:8787" });
