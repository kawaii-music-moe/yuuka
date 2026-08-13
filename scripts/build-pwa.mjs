import { spawnSync } from "node:child_process";
import { cp, rm } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const clientDir = path.join(root, "client", "pwa");
const target = path.join(root, "src", "public", "pwa");
const run = spawnSync("npm run build", {
	cwd: clientDir,
	stdio: "inherit",
	shell: true,
});
if (run.error) throw run.error;
if (run.status !== 0) process.exit(run.status ?? 1);

await rm(target, { recursive: true, force: true });
await cp(path.join(clientDir, "dist"), target, { recursive: true });
