import http from "node:http";
import fs from "node:fs";
import path from "node:path";
import { config } from "./config.js";
import { checkHttps, getSessionUser } from "./server/httpHelpers.js";

// ── ルートレジストリ（全HTTPルートは server/routes/*.ts のモジュールへ分離） ──
import { registerRoutes, dispatchRoute } from "./server/routeRegistry.js";
import { authRoutes } from "./server/routes/authRoutes.js";
import { settingsRoutes } from "./server/routes/settingsRoutes.js";
import { botRoutes } from "./server/routes/botRoutes.js";
import { botAttributeRoutes } from "./server/routes/botAttributeRoutes.js";
import { todoRoutes } from "./server/routes/todoRoutes.js";
import { scheduleRoutes } from "./server/routes/scheduleRoutes.js";
import { financeRoutes } from "./server/routes/financeRoutes.js";
import { playbookRoutes } from "./server/routes/playbookRoutes.js";
import { credentialRoutes } from "./server/routes/credentialRoutes.js";
import { adminRoutes } from "./server/routes/adminRoutes.js";
import { reminderRoutes } from "./server/routes/reminderRoutes.js";
import { personalRoutes } from "./server/routes/personalRoutes.js";
import { personaRoutes } from "./server/routes/personaRoutes.js";
import { mcpRoutes } from "./server/routes/mcpRoutes.js";
import { integratedRoutes } from "./server/routes/integratedRoutes.js";
import { webhookRoutes } from "./server/routes/webhookRoutes.js";
import { deliveryRoutes } from "./server/routes/deliveryRoutes.js";
import { clientRoutes } from "./server/routes/clientRoutes.js";

registerRoutes(clientRoutes);

registerRoutes(authRoutes); // 認証・登録（§5.4）
registerRoutes(settingsRoutes); // ユーザー設定・ステータス・Google OAuth
registerRoutes(botRoutes); // Botインスタンス・共有（§5.1, §5.2）
registerRoutes(botAttributeRoutes); // Bot属性・汎用モード設定（bot_attributes_requirements.md）
registerRoutes(todoRoutes); // ToDo（§3.2）
registerRoutes(scheduleRoutes); // 予定（§3.2）
registerRoutes(financeRoutes); // 家計・予算・支払い予定（§3.4）
registerRoutes(playbookRoutes); // マクロ/Playbook（§3.6）
registerRoutes(credentialRoutes); // パスワードマネージャ（§6）
registerRoutes(adminRoutes); // Admin管理（§5.3）
registerRoutes(reminderRoutes); // リマインド（§3.3）
registerRoutes(personalRoutes); // ノート・クリップボード・連絡先（§3.7, §3.10, §3.11）
registerRoutes(personaRoutes); // ペルソナ・マーケットプレイス（§4.1）
registerRoutes(mcpRoutes); // MCPサーバー拡張（§4.4）
registerRoutes(integratedRoutes); // Bot統合管理（owner単位の横断ページ, v5）
registerRoutes(webhookRoutes); // 外部Webhook受信（§3.13）
registerRoutes(deliveryRoutes); // 朝報・日報・週報（§3.8, §3.9）

// ─── 静的ファイル配信 ────────────────────────────────────────────────────────

const PUBLIC_DIR = path.resolve(process.cwd(), "src", "public");
const PWA_PUBLIC_DIR = path.join(PUBLIC_DIR, "pwa");
const CLIENT_ROUTES = new Set([
	"/",
	"/chat",
	"/todo",
	"/calendar",
	"/finance",
	"/notes",
	"/settings",
]);
const CLIENT_STATIC_PATHS = new Set([
	"/manifest.webmanifest",
	"/sw.js",
	"/icons/app-icon.svg",
]);

// MCP管理ダッシュボード（ywrk-mcp の SPA）は、サンドボックス iframe（sandbox="allow-scripts" のみ＝
// 不透明オリジン）に隔離して埋め込む。iframe 内のドキュメントは専用ルート
// （/api/mcp-servers/:id/dashboard, mcpRoutes.ts）が独自の CSP を付けて返すため、本体（この CSP）は
// akizakura.css 等のダッシュボード固有オリジンを許可する必要はない（iframe 内に閉じる）。
// frame-src は default-src 'self' にフォールバックし、同一オリジンの dashboard ルートを許可する。
// （注意: この CSP を変更したら dist を再ビルドし、稼働中の node プロセスを再起動すること。
//  古いプロセスはメモリ上の旧 CSP を返し続ける。）
// script-src から 'unsafe-inline' を除去（インラインscript/イベントハンドラは全て排除済み）。
// これによりCSPがXSSに対する実効的な多層防御として機能する。インラインJSを追加する場合は
// nonce/hash 方式へ移行すること。style-src の 'unsafe-inline' はテンプレート内の style= のため当面維持。
const CSP =
	"default-src 'self'; script-src 'self' https://static.cloudflareinsights.com; style-src 'self' 'unsafe-inline' https://fonts.googleapis.com https://fonts.gstatic.com; font-src 'self' https://fonts.gstatic.com https://fonts.googleapis.com; img-src 'self' data: https://assets-global.website-files.com https://cdn.discordapp.com; connect-src 'self' https://cloudflareinsights.com; worker-src 'self'; frame-src 'self'; frame-ancestors 'self';";
const SECURITY_HEADERS = {
	"X-Content-Type-Options": "nosniff",
	"X-Frame-Options": "SAMEORIGIN",
	"Referrer-Policy": "strict-origin-when-cross-origin",
	"Content-Security-Policy": CSP,
} as const;

// Mime Types 辞書
const MIME_TYPES: Record<string, string> = {
	".html": "text/html; charset=utf-8",
	".css": "text/css; charset=utf-8",
	".js": "application/javascript; charset=utf-8",
	".png": "image/png",
	".jpg": "image/jpeg",
	".jpeg": "image/jpeg",
	".svg": "image/svg+xml",
".json": "application/json",
	".webmanifest": "application/manifest+json; charset=utf-8",
	".woff2": "font/woff2",
	".webp": "image/webp",
	".ico": "image/x-icon",
};

/**
 * セキュリティヘッダーを設定した静的ファイル配信
 */
function serveStaticFile(req: http.IncomingMessage, res: http.ServerResponse) {
	const urlPath =
		req.url === "/" || !req.url || req.url.startsWith("/?")
			? "/"
			: req.url.split("?")[0];
	const isClientRoute = CLIENT_ROUTES.has(urlPath);
	const isClientAsset =
		CLIENT_STATIC_PATHS.has(urlPath) || urlPath.startsWith("/assets/");
	const isLoginRequest = urlPath === "/login";
	const isAdminRequest =
		isLoginRequest || urlPath === "/admin" || urlPath.startsWith("/admin/");
	if (!isClientRoute && !isClientAsset && !isAdminRequest) {
		res.writeHead(404, { "Content-Type": "text/plain; charset=utf-8" });
		res.end("404 Not Found");
		return;
	}
	const staticRoot = isClientRoute || isClientAsset ? PWA_PUBLIC_DIR : PUBLIC_DIR;
	const relativePath = isClientRoute
		? "/index.html"
		: isLoginRequest
			? "/index.html"
			: isAdminRequest
			? (urlPath.slice("/admin".length) || "/index.html")
			: urlPath;

	// セキュリティ対策：パス・トラバーサルの防御
	const resolvedPath = path.normalize(path.join(staticRoot, relativePath));

	if (
		!resolvedPath.startsWith(staticRoot + path.sep) &&
		resolvedPath !== staticRoot
	) {
		res.writeHead(403, { "Content-Type": "text/plain" });
		res.end("403 Forbidden");
		return;
	}

	fs.stat(resolvedPath, (err, stats) => {
		let finalPath = resolvedPath;
		if (err || !stats.isFile()) {
			const ext = path.extname(resolvedPath);
			// SPAのパスルーティング（拡張子なしのパス）の場合は、index.htmlを配信する
			if (!ext) {
				finalPath = path.join(staticRoot, "index.html");
			} else {
				res.writeHead(404, { "Content-Type": "text/plain" });
				res.end("404 Not Found");
				return;
			}
		}

		const ext = path.extname(finalPath).toLowerCase();
		const contentType = MIME_TYPES[ext] || "application/octet-stream";

		res.writeHead(200, {
			"Content-Type": contentType,
			"Cache-Control": "no-cache, no-store, must-revalidate",
			...SECURITY_HEADERS,
		});

		if (ext === ".html" && path.basename(finalPath) === "index.html") {
			fs.readFile(finalPath, "utf-8", (err2, content) => {
				if (err2) {
					console.error("Failed to read index.html:", err2);
					res.end("Internal Server Error");
					return;
				}
				let html = content;
				if (isAdminRequest) {
					html = html
						.replaceAll('href="/', 'href="/admin/')
						.replaceAll('src="/', 'src="/admin/');
				}
				if (config.googleSiteVerification) {
					const metaTag = `<meta name="google-site-verification" content="${config.googleSiteVerification}" />`;
					html = html.replace("<!-- GOOGLE_SITE_VERIFICATION -->", metaTag);
				} else {
					html = html.replace("<!-- GOOGLE_SITE_VERIFICATION -->", "");
				}
				res.end(html);
			});
			return;
		}

		// 'error' リスナーが無いとストリームのエラーイベントでプロセスごとクラッシュする
		// （stat 後にファイルが消えた場合や読み取りエラー等）
		const stream = fs.createReadStream(finalPath);
		stream.on("error", (streamErr) => {
			console.error(
				`静的ファイルの読み取りに失敗しました (${finalPath}):`,
				streamErr,
			);
			res.destroy();
		});
		res.on("close", () => {
			stream.destroy(); // クライアント切断時に読み取りを確実に打ち切る
		});
		stream.pipe(res);
	});
}

// ─── メインハンドラー ────────────────────────────────────────────────────────

/**
 * Webサーバーのメインハンドラー。
 * HTTPSリダイレクト・CORS制御の後、ルートレジストリへディスパッチし、
 * どのルートにも合致しないリクエストは静的ファイル（SPA）として配信する。
 */
export async function serverHandler(
	req: http.IncomingMessage,
	res: http.ServerResponse,
) {
	const { url } = req;
	const parsedUrl = new URL(
		url || "/",
		`http://${req.headers.host || "localhost"}`,
	);
	const pathname = parsedUrl.pathname;
	const legacyAdminPaths = ["/bot", "/bots", "/usage", "/terms", "/privacy"];
	if (legacyAdminPaths.some((path) => pathname === path || pathname.startsWith(`${path}/`))) {
		res.writeHead(302, { Location: `/admin${url || "/"}` });
		res.end();
		return;
	}

	// 1. HTTPからHTTPSへのリダイレクト判定
	if (config.baseUrl && config.baseUrl.toLowerCase().startsWith("https://")) {
		const isHttps = checkHttps(req);
		if (!isHttps) {
			const baseUrlObj = new URL(config.baseUrl);
			const reqHost = req.headers.host;
			if (reqHost) {
				const reqHostName = reqHost.split(":")[0];
				if (reqHostName === baseUrlObj.hostname) {
					// HTTPSのURLに301リダイレクト
					res.writeHead(301, {
						Location: `https://${baseUrlObj.hostname}${url || ""}`,
					});
					res.end();
					return;
				}
			}
		}
		// HSTS: HTTPS本番デプロイ時は全レスポンスに付与する（SSLストリッピング対策）。
		// setHeader で先に積むことで、以降の writeHead（静的配信・API・エラー）すべてに反映される。
		// 2年・サブドメイン込み。preload は申請が伴うため運用判断に委ねて含めない。
		res.setHeader(
			"Strict-Transport-Security",
			"max-age=63072000; includeSubDomains",
		);
	}

	// 2. CORS対応 (ローカル接続または信頼されたオリジンに限定)
	const requestOrigin = req.headers.origin;
	if (requestOrigin) {
		let isAllowedOrigin = false;
		// localhost オリジンの許可は baseUrl 未設定（ローカル開発）時のみ。
		// 本番（baseUrl 設定済み）でローカルの悪性プロセスからの資格情報付き
		// クロスオリジンを反射しないようにする
		if (
			!config.baseUrl &&
			(requestOrigin.startsWith("http://localhost:") ||
				requestOrigin.startsWith("http://127.0.0.1:"))
		) {
			isAllowedOrigin = true;
		} else if (config.baseUrl) {
			try {
				const baseUrlObj = new URL(config.baseUrl);
				const originUrlObj = new URL(requestOrigin);
				if (originUrlObj.hostname === baseUrlObj.hostname) {
					isAllowedOrigin = true;
				}
			} catch {}
		}

		if (isAllowedOrigin) {
			res.setHeader("Access-Control-Allow-Origin", requestOrigin);
			res.setHeader("Access-Control-Allow-Credentials", "true");
		}
	}

	// 3. ルートレジストリへディスパッチ（認可・ボディ解析はレジストリが担当）
	//    （MCP ダッシュボードは隔離 iframe = 不透明オリジンで動くため /proxy/mcp/ への呼び出しは
	//     クロスオリジンになるが、その CORS プリフライト(OPTIONS)とACAO:null は mcpRoutes 側の
	//     専用ルートが処理する。ここで特別扱いする必要はない。）
	try {
		const handled = await dispatchRoute(req, res, parsedUrl, () =>
			getSessionUser(req),
		);
		if (handled) return;
	} catch (err) {
		console.error("ルートレジストリ処理エラー:", err);
		if (!res.headersSent) {
			res.writeHead(500, {
				"Content-Type": "application/json",
				...SECURITY_HEADERS,
			});
			res.end(
				JSON.stringify({
					success: false,
					message: "内部エラーが発生しました。",
				}),
			);
		}
		return;
	}

	// 4. 未登録の /api/* は404、それ以外は静的ファイル（SPA）として配信
	if (pathname.startsWith("/api/")) {
		res.writeHead(404, {
			"Content-Type": "application/json",
			...SECURITY_HEADERS,
		});
		res.end(
			JSON.stringify({
				success: false,
				message: "APIエンドポイントが見つかりません。",
			}),
		);
		return;
	}

	serveStaticFile(req, res);
}

// ─── サーバー起動・停止 ──────────────────────────────────────────────────────

let server: http.Server | null = null;

/**
 * Webサーバーの起動
 */
export function startWebServer(): Promise<void> {
	return new Promise((resolve) => {
		server = http.createServer((req, res) => {
			serverHandler(req, res).catch((err) => {
				console.error("サーバーハンドラで予期しないエラー:", err);
				if (!res.headersSent) {
					res.writeHead(500, {
						"Content-Type": "application/json",
						...SECURITY_HEADERS,
					});
					res.end(
						JSON.stringify({
							success: false,
							message: "内部エラーが発生しました。",
						}),
					);
				}
			});
		});

		server.listen(config.port, config.host, () => {
			console.log(
				`🌐 Yuuka 管理画面サーバー起動完了: http://${config.host}:${config.port}`,
			);
			resolve();
		});
	});
}

/**
 * Webサーバーの停止
 */
export function stopWebServer(): void {
	if (server) {
		server.close(() => {
			console.log("🌐 Yuuka 管理画面サーバーを停止しました。");
		});
		server = null;
	}
}
