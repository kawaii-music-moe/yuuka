// PWA と管理画面の Vite を前景で起動する（`pnpm dev`。systemd の dev:start からも使う）。
import {
	clientDir,
	installHint,
	processGroup,
	root,
} from "./lib/processes.mjs";

const api = process.env.VITE_API_TARGET ?? "http://127.0.0.1:3403";
const host = process.env.VITE_BIND_ADDR ?? "127.0.0.1";
const adminPort = process.env.VITE_PORT ?? "7855";
const clientPort = process.env.VITE_CLIENT_PORT ?? "5173";
const group = processGroup();
const listen = (config, port) => [
	"--config",
	config,
	"--host",
	host,
	"--port",
	port,
	"--strictPort",
];

try {
	group.vite(root, listen("frontend/vite.config.ts", adminPort), {
		VITE_API_TARGET: api,
		VITE_PWA_DEV_SERVER: `http://127.0.0.1:${clientPort}`,
	});
	group.vite(clientDir, listen("vite.config.ts", clientPort), {
		VITE_API_PROXY_TARGET: api,
		VITE_ADMIN_DEV_SERVER: `http://127.0.0.1:${adminPort}`,
	});
	console.log(
		`Dev: http://127.0.0.1:${clientPort}/ (admin: /admin/, API: ${api})`,
	);
} catch (error) {
	console.error(`${error.message}\n${installHint}`);
	group.shutdown(1);
}
