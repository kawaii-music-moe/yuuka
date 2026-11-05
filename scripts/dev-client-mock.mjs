// モック API（clients/pwa/mock/server.ts・8787）で PWA と管理画面を起動する（`pnpm dev:client:mock`）。
//
// 管理画面（frontend の Svelte dev server・/admin/ 配下）は 5174 に固定する。Client の
// /admin/... はここへプロキシされ、/login は /admin/login へリダイレクトされる
// （clients/pwa/vite.config.ts 参照）。ログイン後の returnTo が Client のパスなら
// 逆に Client へ戻される（frontend/vite.config.ts の VITE_PWA_DEV_SERVER）。
// /api は両方ともこのモック API へ向ける（#44）。
import { clientDir, installHint, processGroup, root } from "./lib/processes.mjs";

const mockApi = "http://localhost:8787";
const group = processGroup();

try {
	group.start(process.platform === "win32" ? "npm.cmd" : "npm", ["run", "mock"], {
		cwd: clientDir,
		shell: process.platform === "win32",
	});
	group.vite(clientDir, [], {
		VITE_API_PROXY_TARGET: mockApi,
		VITE_ADMIN_DEV_SERVER: "http://localhost:5174",
	});
	group.vite(
		root,
		["--config", "frontend/vite.config.ts", "--port", "5174", "--strictPort"],
		{
			VITE_API_TARGET: mockApi,
			VITE_PWA_DEV_SERVER: "http://localhost:5173",
		},
	);
	console.log("Dev (mock API): http://localhost:5173/ (admin: /admin/)");
} catch (error) {
	console.error(`${error.message}\n${installHint}`);
	group.shutdown(1);
}
