<script lang="ts">
	// ─────────────────────────────────────────────────────────────────────────
	// App.svelte — 本配線（統合担当）
	//
	// 責務:
	//   - ルーター初期化（popstate 1個）+ テーマストア初期化 + bootstrapSession()
	//   - §12.3 theme-no-transition 除去
	//   - §8 ルーティング対応表に従った単一表示ソースの出し分け
	//     （CSS .active との二重管理を避け {#if}/ルーターへ一元化）
	//   - 認証ゲート / admin ガード / Bot 未選択リダイレクト（ルーターに密結合せず
	//     ここで認証ストア購読ガードとして実施 — §8 末尾 / §9）
	//   - Toast / ConfirmDialog をルート直下に常設
	// ─────────────────────────────────────────────────────────────────────────
	import { onMount, onDestroy } from "svelte";

	// theme ストアは import するだけで subscribe 副作用（data-theme/localStorage/
	// meta[theme-color] 同期）が走る設計。
	import "$lib/stores/theme";
	import { isAuthed, isAdmin, bootstrapSession } from "$lib/stores/session";
	import { activeBot } from "$lib/stores/activeBot";
	import {
		page,
		initRouter,
		resolveRoute,
		type ResolvedRoute,
		type RouteView,
	} from "$lib/router";

	// effectiveView は RouteView に「認証保留中」の "loading" を足した表示状態。
	type EffectiveView = RouteView | "loading";

	import { Toast, ConfirmDialog, LazyView } from "$lib/components/ui";
	import AdminNavigationDrawer from "$lib/components/AdminNavigationDrawer.svelte";

	// ルート系オーバーレイ/ページ
	// 初期表示で頻繁に踏む Login / BotSelection / BotShell は静的 import で据え置き
	// （初回描画の往復を増やさない）。
	import Login from "./overlays/Login.svelte";
	import BotSelection from "./overlays/BotSelection.svelte";
	import BotShell from "./routes/BotShell.svelte";

	// §P1b: 重い/低頻度のオーバーレイは遅延ロード（loader を LazyView に渡す）。
	// Vite が各 import() を個別チャンク（AdminOverlay-*.js 等）に分割し、初期
	// entry（index-*.js）から実体が外れる。Promise 安定化は LazyView 内部の
	// $derived(loader()) に集約する。
	const loadIntegrated = () => import("./overlays/IntegratedOverlay.svelte");
	const loadAdmin = () => import("./overlays/AdminOverlay.svelte");
	const loadAccount = () => import("./overlays/AccountOverlay.svelte");
	const loadDevice = () => import("./overlays/DeviceOverlay.svelte");
	const loadUsage = () => import("./overlays/Usage.svelte");
	const loadTerms = () => import("./overlays/Terms.svelte");
	const loadPrivacy = () => import("./overlays/Privacy.svelte");
	const loadTasksGuide = () => import("./overlays/TasksGuide.svelte");

	// ── 状態購読 ─────────────────────────────────────────────────────────────
	// currentRoute（cleanPath）と page（URL）は両方 router が更新する。
	// resolveRoute は page（searchParams 保持）を優先入力にする。
	let resolved = $state<ResolvedRoute>(resolveRoute($page));
	let authed = $state(false);
	let admin = $state(false);

	page.subscribe((u) => (resolved = resolveRoute(u)));
	isAuthed.subscribe((v) => (authed = v));
	isAdmin.subscribe((v) => (admin = v));

	// セッションブートストラップ完了フラグ。完了までは認証依存ルートで
	// 未ログインへ即バウンスさせない（/api/me 応答待ち）。
	let bootstrapped = $state(false);

	// ── 認証ゲート付き実効ビュー ─────────────────────────────────────────────
	// §8 対応表を単一の $derived に集約。ここが唯一の表示決定ソース。
	const PUBLIC_VIEWS = new Set(["usage", "terms", "privacy", "tasks-guide"]);

	const effectiveView = $derived.by((): EffectiveView => {
		const v = resolved.view;

		// 公開ルートは認証前でも常に描画。
		if (PUBLIC_VIEWS.has(v)) return v;

		// /device はログイン往復に対応するため、そのまま描画（未ログインでも
		// Login を挟まず承認画面を出し、内部でログイン誘導する設計）。
		if (v === "device") return v;

		// login 画面はゲート対象外。
		if (v === "login") return authed ? "bots" : "login";

		// ここから要ログイン領域。ブートストラップ未完了なら判定を保留（何も出さない）。
		if (!bootstrapped) return "loading";
		if (!authed) return "login";

		// admin ガード: 非 admin は Bot 選択へ。
		if (v === "admin") return admin ? "admin" : "bots";

		// Bot 個別画面: Bot 未選択なら選択画面へ。
		if (v === "bot") return $activeBot ? "bot" : "bots";

		// notfound: ログイン済は Bot 選択へ丸める。
		if (v === "notfound") return "bots";

		// integrated / account / bots はそのまま。
		return v;
	});

	// ── 初期化 ───────────────────────────────────────────────────────────────
	let cleanupRouter: (() => void) | null = null;

	onMount(() => {
		cleanupRouter = initRouter();

		// §12.3: 初回描画確定後にトランジション抑止クラスを外す。
		document.documentElement.classList.remove("theme-no-transition");

		// セッション取得（未ログインは匿名 = currentUser null のまま）。
		void bootstrapSession().finally(() => {
				bootstrapped = true;
		});
	});

	onDestroy(() => {
		cleanupRouter?.();
	});
</script>

<!--
  単一表示ソース: effectiveView（§8 対応表 + 認証ゲート）で一元的に出し分ける。
  CSS の .active クラスによる旧オーバーレイ排他は使わない（§15）。
-->
{#if effectiveView === "loading"}
	<div class="app-boot" aria-busy="true"></div>
{:else if effectiveView === "login"}
	<Login />
{:else if effectiveView === "bots"}
	<BotSelection />
{:else if effectiveView === "bot"}
	<BotShell tab={resolved.tab ?? "config"} />
{:else if effectiveView === "integrated"}
	<LazyView loader={loadIntegrated} />
{:else if effectiveView === "admin"}
	<LazyView loader={loadAdmin} />
{:else if effectiveView === "account"}
	<LazyView loader={loadAccount} />
{:else if effectiveView === "device"}
	<LazyView loader={loadDevice} />
{:else if effectiveView === "usage"}
	<LazyView loader={loadUsage} />
{:else if effectiveView === "terms"}
	<LazyView loader={loadTerms} />
{:else if effectiveView === "privacy"}
	<LazyView loader={loadPrivacy} />
{:else if effectiveView === "tasks-guide"}
	<LazyView loader={loadTasksGuide} />
{/if}

<!-- ルート直下に一度だけ常設（alert()/confirm() 置換） -->
<Toast />
<ConfirmDialog />
{#if authed}
	<AdminNavigationDrawer />
{/if}

<style>
	.app-boot {
		min-height: 100vh;
	}
</style>
