import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import http from "node:http";
import net from "node:net";
import { test } from "node:test";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));

async function listen(server) {
	server.listen(0, "127.0.0.1");
	await once(server, "listening");
	return server.address().port;
}

async function unusedPort() {
	const server = net.createServer();
	const port = await listen(server);
	await new Promise((resolve) => server.close(resolve));
	return port;
}

function start(adminPort, clientPort, apiPort) {
	const child = spawn(process.execPath, ["scripts/dev.mjs"], {
		cwd: root,
		env: {
			...process.env,
			VITE_PORT: String(adminPort),
			VITE_CLIENT_PORT: String(clientPort),
			VITE_BIND_ADDR: "127.0.0.1",
			VITE_API_TARGET: `http://127.0.0.1:${apiPort}`,
		},
		stdio: ["ignore", "pipe", "pipe"],
	});
	let output = "";
	child.stdout.on("data", (data) => {
		output += data;
	});
	child.stderr.on("data", (data) => {
		output += data;
	});
	return { child, exited: once(child, "exit"), output: () => output };
}

async function ready(url, running) {
	for (let i = 0; i < 100; i++) {
		assert.equal(running.child.exitCode, null, running.output());
		try {
			const response = await fetch(url);
			if (response.ok) return response;
		} catch {
			/* server is starting */
		}
		await delay(100);
	}
	assert.fail(`Timed out: ${url}\n${running.output()}`);
}

async function assertClosed(port) {
	await assert.rejects(fetch(`http://127.0.0.1:${port}/`));
}

test("both frontends serve HMR and proxy API; SIGTERM stops both", {
	timeout: 20000,
}, async (t) => {
	const api = http.createServer((_request, response) => {
		response.writeHead(401, { "Content-Type": "application/json" });
		response.end('{"source":"test-api"}');
	});
	const apiPort = await listen(api);
	t.after(() => api.close());
	const adminPort = await unusedPort();
	const clientPort = await unusedPort();
	const running = start(adminPort, clientPort, apiPort);
	t.after(() => running.child.kill("SIGTERM"));
	const base = `http://127.0.0.1:${clientPort}`;
	await ready(`${base}/`, running);
	await ready(`${base}/admin/`, running);
	for (const path of [
		"/@vite/client",
		"/admin/@vite/client",
		"/src/main.ts",
		"/admin/src/main.ts",
	]) {
		const response = await fetch(base + path);
		assert.equal(response.status, 200, path);
		assert.match(response.headers.get("content-type"), /javascript/, path);
	}
	for (const port of [adminPort, clientPort]) {
		const response = await fetch(`http://127.0.0.1:${port}/api/me`);
		assert.equal(response.status, 401);
		assert.deepEqual(await response.json(), { source: "test-api" });
	}
	running.child.kill("SIGTERM");
	const [code] = await running.exited;
	assert.equal(code, 0, running.output());
	await assertClosed(adminPort);
	await assertClosed(clientPort);
});

test("a port collision exits unsuccessfully and stops the sibling", {
	timeout: 20000,
}, async (t) => {
	const occupied = net.createServer();
	const clientPort = await listen(occupied);
	t.after(() => occupied.close());
	const adminPort = await unusedPort();
	const running = start(adminPort, clientPort, 1);
	t.after(() => running.child.kill("SIGTERM"));
	const [code] = await running.exited;
	assert.notEqual(code, 0, running.output());
	assert.match(running.output(), /already in use/);
	await assertClosed(adminPort);
});
