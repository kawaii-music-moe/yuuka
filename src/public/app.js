/* ==============================================================================
   Personal Assistant Dashboard - パーソナルアシスタント管理室 - Core Frontend JS Engine
   Secure Vanilla Javascript, Zero-Dependency, Pure DOM elements & SSE/REST
   Upgraded UI Logic: Custom Interactive Crypto Sparklines and Line Charts
   ============================================================================== */

document.addEventListener("DOMContentLoaded", () => {
	document.documentElement.classList.remove("theme-no-transition");

	// HTMLエスケープ（element-text / 二重・一重引用属性のいずれでも安全）。Stored-XSS対策の共通ヘルパー。
	function escapeHtml(s) {
		return String(s == null ? "" : s).replace(
			/[&<>"']/g,
			(c) =>
				({
					"&": "&amp;",
					"<": "&lt;",
					">": "&gt;",
					'"': "&quot;",
					"'": "&#39;",
				})[c],
		);
	}

	// Override native fetch to auto-inject currentBotId
	const originalFetch = window.fetch;
	window.fetch = async function (resource, options) {
		if (
			typeof resource === "string" &&
			resource.startsWith("/api/") &&
			window.currentBotId
		) {
			if (
				!resource.startsWith("/api/login") &&
				!resource.startsWith("/api/register") &&
				!resource.startsWith("/api/logout") &&
				!resource.startsWith("/api/bots") &&
				!resource.startsWith("/api/me")
			) {
				const url = new URL(resource, window.location.origin);
				if (!url.searchParams.has("botId")) {
					url.searchParams.set("botId", window.currentBotId);
				}
				resource = url.pathname + url.search;

				if (
					options &&
					options.body &&
					typeof options.body === "string" &&
					!options.body.startsWith("-----")
				) {
					try {
						const bodyObj = JSON.parse(options.body);
						if (!bodyObj.botId) {
							bodyObj.botId = window.currentBotId;
							options.body = JSON.stringify(bodyObj);
						}
					} catch (e) {
						// Ignore parsing errors for non-JSON bodies
					}
				}
			}
		}
		return originalFetch(resource, options);
	};

	// Theme management
	const THEME_KEY = "yuuka-theme";
	const THEME_META_COLORS = {
		dark: "#121212",
		light: "#FAFAFA",
		"blue-archive": "#FBFCFF",
	};
	const savedTheme = localStorage.getItem(THEME_KEY) || "dark";

	const btnThemeToggle = document.getElementById("btn-theme-toggle");
	const themeIcon = document.getElementById("theme-icon");

	function currentTheme() {
		return document.documentElement.getAttribute("data-theme") || "dark";
	}

	function applyThemeIcon(theme) {
		themeIcon.textContent = theme === "dark" ? "light_mode" : "dark_mode";
		btnThemeToggle.title =
			theme === "dark" ? "ライトテーマに切り替え" : "ダークテーマに切り替え";
	}

	function applyTheme(theme) {
		document.documentElement.setAttribute("data-theme", theme);
		localStorage.setItem(THEME_KEY, theme);
		applyThemeIcon(theme);
		const metaThemeColor = document.querySelector('meta[name="theme-color"]');
		if (metaThemeColor) {
			metaThemeColor.setAttribute(
				"content",
				THEME_META_COLORS[theme] || THEME_META_COLORS.dark,
			);
		}
		document.querySelectorAll(".theme-option").forEach((btn) => {
			btn.classList.toggle("active", btn.getAttribute("data-theme") === theme);
		});
	}

	applyTheme(savedTheme);

	// ヘッダーのトグルはダーク⇔ライトを切り替え（BAテーマからはダークへ戻る）
	btnThemeToggle.addEventListener("click", () => {
		applyTheme(currentTheme() === "dark" ? "light" : "dark");
	});

	// 設定タブのテーマセレクタ（ダーク / ライト / BA）
	document.querySelectorAll(".theme-option").forEach((btn) => {
		btn.addEventListener("click", () =>
			applyTheme(btn.getAttribute("data-theme")),
		);
	});

	// State management
	let activeTab = "dashboard";
	let activeUserId = "";
	let activeUserRole = "user";
	let pendingTasksCount = 0;
	let totalExpensesVal = 0;

	// Cache DOM Elements
	const loginOverlay = document.getElementById("login-overlay");
	const loginForm = document.getElementById("login-form");
	const loginError = document.getElementById("login-error");
	const appContainer = document.getElementById("app-container");

	// Bot Selection Overlay DOM elements
	const botSelectionOverlay = document.getElementById("bot-selection-overlay");
	const botList = document.getElementById("bot-list");
	const btnShowAddBot = document.getElementById("btn-show-add-bot");
	const btnBotLogout = document.getElementById("btn-bot-logout");
	const modalCreateBot = document.getElementById("modal-create-bot");
	const createBotForm = document.getElementById("create-bot-form");
	const btnCloseCreateBot = document.getElementById("btn-close-create-bot");
	const btnSwitchBot = document.getElementById("btn-switch-bot");
	const activeBotDisplay = document.getElementById("active-bot-display");

	// Login / Register Tabs
	const btnTabLogin = document.getElementById("btn-tab-login");
	const btnTabRegister = document.getElementById("btn-tab-register");
	const loginTabContent = document.getElementById("login-tab-content");
	const registerTabContent = document.getElementById("register-tab-content");
	const registerForm = document.getElementById("register-form");

	const btnLogout = document.getElementById("btn-logout");

	const currentTabTitle = document.getElementById("current-tab-title");
	const menuItems = document.querySelectorAll(".menu-item");
	const tabViews = document.querySelectorAll(".tab-view");

	// 秘書プリセット専用のタブ（汎用モードBotの文脈では非表示にする）
	const SECRETARY_ONLY_TABS = [
		"tasks",
		"schedules",
		"expenses",
		"reminders",
		"personal",
		"delivery",
		"webhooks",
		"playbooks",
	];

	function currentBotPreset() {
		return localStorage.getItem("currentBotPreset") || "secretary";
	}

	function updateSidebarBotBranding() {
		const sidebarAppAvatar = document.getElementById("sidebar-app-avatar");
		const sidebarAppName = document.getElementById("sidebar-app-name");
		const sidebarAppId = document.getElementById("sidebar-app-id");

		if (!sidebarAppName || !sidebarAppId) return;

		const botName =
			localStorage.getItem("currentBotName") || "システムデフォルト";
		const botId = localStorage.getItem("currentBotId") || "system_default";
		const botAvatar = localStorage.getItem("currentBotAvatar") || "";

		sidebarAppName.textContent = botName;
		sidebarAppId.textContent = botId.startsWith("bot_")
			? botId.substring(4)
			: botId;

		if (sidebarAppAvatar) {
			if (botAvatar) {
				sidebarAppAvatar.src = botAvatar;
				sidebarAppAvatar.style.display = "block";
			} else {
				sidebarAppAvatar.src =
					"https://assets-global.website-files.com/6257adef93867e50d84d30e2/636e0a6a49cf127bf92de1e2_icon_clyde_blurple_RGB.png";
				sidebarAppAvatar.style.display = "block";
			}
		}

		// 属性に含まれない機能のタブは、そのBotの文脈では非表示にする（Bot属性 §4.7）
		const isAssistant = currentBotPreset() === "mcp_assistant";
		menuItems.forEach((item) => {
			const tab = item.getAttribute("data-tab");
			if (SECRETARY_ONLY_TABS.includes(tab)) {
				item.style.display = isAssistant ? "none" : "";
			}
		});

		// ダッシュボード内の秘書系セクション / 汎用モードのAPI使用量セクションを切り替える
		applyDashboardMode(isAssistant);
	}

	/**
	 * ダッシュボードの表示モードを現在のBotプリセットに合わせて切り替える。
	 * 汎用モード(mcp_assistant)では財務・タスク・予定など無効な機能のカードを隠し、
	 * 代わりにAPI使用量チャートを表示する（Bot属性 §4.7）。
	 */
	function applyDashboardMode(isAssistant) {
		const assistant =
			typeof isAssistant === "boolean"
				? isAssistant
				: currentBotPreset() === "mcp_assistant";
		document.querySelectorAll(".dashboard-secretary-only").forEach((el) => {
			el.style.display = assistant ? "none" : "";
		});
		document.querySelectorAll(".dashboard-assistant-only").forEach((el) => {
			el.style.display = assistant ? "" : "none";
		});
	}

	// Dashboard DOM
	const yuukaBubbleText = document.getElementById("yuuka-bubble-text");
	const statPendingTasks = document.getElementById("stat-pending-tasks");
	const statUpcomingSchedules = document.getElementById(
		"stat-upcoming-schedules",
	);
	const statExpensesTotal = document.getElementById("stat-expenses-total");
	const donutSegment = document.getElementById("donut-segment");
	const chartCenterPercentage = document.getElementById(
		"chart-center-percentage",
	);
	const dashboardCategoryLegend = document.getElementById(
		"dashboard-category-legend",
	);
	const dashboardUrgentList = document.getElementById("dashboard-urgent-list");
	const tasksList = document.getElementById("tasks-list");
	const schedulesList = document.getElementById("schedules-list");

	// Modal DOM
	const modalTask = document.getElementById("modal-task");
	const modalSchedule = document.getElementById("modal-schedule");
	const modalReceiptResult = document.getElementById("modal-receipt-result");
	const modalCredential = document.getElementById("modal-credential");
	const taskForm = document.getElementById("task-form");
	const credentialForm = document.getElementById("credential-form");
	const scheduleForm = document.getElementById("schedule-form");
	const receiptAiResponse = document.getElementById("receipt-ai-response");

	// Expenses Tab DOM
	const expenseMonthTotal = document.getElementById("expense-month-total");
	const receiptDropzone = document.getElementById("receipt-dropzone");
	const receiptFileInput = document.getElementById("receipt-file-input");
	const scanStatus = document.getElementById("scan-status");
	const scanStatusText = document.getElementById("scan-status-text");
	const expenseForm = document.getElementById("expense-form");
	const expensesTableBody = document.getElementById("expenses-table-body");

	// Setup Date inputs defaults
	const expDateInput = document.getElementById("exp-date");
	if (expDateInput) {
		expDateInput.value = new Date().toISOString().slice(0, 10);
	}

	// Handle Google OAuth callback notifications
	const urlParams = new URLSearchParams(window.location.search);
	const oauthStatus = urlParams.get("oauth");
	if (oauthStatus === "success") {
		const note = urlParams.get("note");
		if (note === "existing_token_used") {
			alert(
				"🎉 Googleアカウント連携が完了しました！（既存の接続を使用します）",
			);
		} else {
			alert("🎉 Googleアカウントとの認証連携に成功しました！");
		}
		window.history.replaceState({}, document.title, window.location.pathname);
	} else if (oauthStatus === "error") {
		const errorMsg = urlParams.get("msg") || "未知のエラー";
		alert(`❌ Google連携認証に失敗しました:\n${errorMsg}`);
		window.history.replaceState({}, document.title, window.location.pathname);
	}

	// ==========================================
	// VIEW ROUTER (HTML5 History Path-based Routing)
	// ==========================================
	function switchTab(tabId) {
		// 汎用モードBotでは秘書系タブへ遷移させない（Bot設定へフォールバック）
		if (
			currentBotPreset() === "mcp_assistant" &&
			SECRETARY_ONLY_TABS.includes(tabId)
		) {
			tabId = "config";
		}
		activeTab = tabId;

		// Update menu buttons active state
		menuItems.forEach((item) => {
			if (item.getAttribute("data-tab") === tabId) {
				item.classList.add("active");
			} else {
				item.classList.remove("active");
			}
		});

		// Update visible views
		tabViews.forEach((view) => {
			if (view.id === `tab-${tabId}`) {
				view.classList.add("active");
			} else {
				view.classList.remove("active");
			}
		});

		// Update Header text
		const titles = {
			dashboard: "ダッシュボード",
			tasks: "タスク管理",
			schedules: "予定スケジュール",
			expenses: "家計管理",
			reminders: "リマインダー",
			personal: "メモ・連絡先",
			personas: "ペルソナ管理",
			delivery: "配信設定",
			webhooks: "Webhook 連携",
			mcp: "MCPサーバー管理",
			playbooks: "Playbook 設定",
			discord: "Discord 連携設定",
			config: "システム設定情報",
		};
		currentTabTitle.textContent = titles[tabId] || "ダッシュボード";

		// Trigger data load
		loadDataForActiveTab();
	}

	function navigateTo(path, pushState = true) {
		const adminPath = path.startsWith("/admin")
			? path
			: `/admin${path.startsWith("/") ? path : `/${path}`}`;
		if (pushState) {
			window.history.pushState({}, "", adminPath);
		}
		applyRoute(adminPath);
	}

	function applyRoute(path) {
		const cleanPath =
			path
				.split("?")[0]
				.split("#")[0]
				.replace(/^\/admin(?=\/|$)/, "")
				.replace(/\/$/, "") || "/";

		const usageOverlay = document.getElementById("usage-overlay");
		if (usageOverlay) usageOverlay.classList.remove("active");
		const termsOverlay = document.getElementById("terms-overlay");
		if (termsOverlay) termsOverlay.classList.remove("active");
		const privacyOverlay = document.getElementById("privacy-overlay");
		if (privacyOverlay) privacyOverlay.classList.remove("active");
		const integratedOverlay = document.getElementById("integrated-overlay");
		if (integratedOverlay) integratedOverlay.classList.remove("active");
		const adminOverlay = document.getElementById("admin-overlay");
		if (adminOverlay) adminOverlay.classList.remove("active");
		const accountOverlay = document.getElementById("account-overlay");
		if (accountOverlay) accountOverlay.classList.remove("active");

		// 全体管理（owner/システム）・アカウント管理の独立オーバーレイ。
		// Bot個別画面（#app-container）の外で表示する。
		if (
			cleanPath === "/integrated" ||
			cleanPath === "/admin" ||
			cleanPath === "/account"
		) {
			if (!activeUserId) {
				initAppSession();
				return;
			}
			if (cleanPath === "/admin" && activeUserRole !== "admin") {
				navigateTo("/", false);
				return;
			}
			loginOverlay.classList.remove("active");
			appContainer.classList.add("hidden");
			botSelectionOverlay.classList.remove("active");
			if (cleanPath === "/integrated") {
				if (integratedOverlay) integratedOverlay.classList.add("active");
				fetchIntegratedOverview();
			} else if (cleanPath === "/account") {
				if (accountOverlay) accountOverlay.classList.add("active");
				fetchAccountSettings();
			} else {
				if (adminOverlay) adminOverlay.classList.add("active");
				fetchAdminData();
			}
			return;
		}

		if (cleanPath === "/usage") {
			appContainer.classList.add("hidden");
			botSelectionOverlay.classList.remove("active");
			loginOverlay.classList.remove("active");
			const botSetupTabContent = document.getElementById(
				"bot-setup-tab-content",
			);
			if (botSetupTabContent) botSetupTabContent.classList.remove("active");
			if (usageOverlay) usageOverlay.classList.add("active");
			return;
		}

		if (cleanPath === "/terms") {
			appContainer.classList.add("hidden");
			botSelectionOverlay.classList.remove("active");
			loginOverlay.classList.remove("active");
			const botSetupTabContent = document.getElementById(
				"bot-setup-tab-content",
			);
			if (botSetupTabContent) botSetupTabContent.classList.remove("active");
			if (termsOverlay) termsOverlay.classList.add("active");
			return;
		}

		if (cleanPath === "/privacy") {
			appContainer.classList.add("hidden");
			botSelectionOverlay.classList.remove("active");
			loginOverlay.classList.remove("active");
			const botSetupTabContent = document.getElementById(
				"bot-setup-tab-content",
			);
			if (botSetupTabContent) botSetupTabContent.classList.remove("active");
			if (privacyOverlay) privacyOverlay.classList.add("active");
			return;
		}

		if (cleanPath === "/login") {
			appContainer.classList.add("hidden");
			botSelectionOverlay.classList.remove("active");
			loginOverlay.classList.add("active");
			const botSetupTabContent = document.getElementById(
				"bot-setup-tab-content",
			);
			if (botSetupTabContent) botSetupTabContent.classList.remove("active");
			return;
		}

		// Bot選択画面 = ルート "/"（旧 "/bots" はエイリアスとして維持）
		if (
			cleanPath === "/" ||
			cleanPath === "/bots" ||
			cleanPath === "/index.html"
		) {
			if (!activeUserId) {
				initAppSession();
				return;
			}
			loginOverlay.classList.remove("active");
			appContainer.classList.add("hidden");
			botSelectionOverlay.classList.add("active");
			fetchBotList();
			return;
		}

		// Bot個別の管理画面 = "/bot"（既定タブ）/ "/bot/<tab>"（例: /bot/personas, /bot/mcp）
		if (cleanPath === "/bot" || cleanPath.startsWith("/bot/")) {
			if (!activeUserId) {
				initAppSession();
				return;
			}
			const botId = localStorage.getItem("currentBotId");
			const botName = localStorage.getItem("currentBotName");
			if (!botId) {
				navigateTo("/", false);
				return;
			}
			const BOT_TABS = [
				"dashboard",
				"reminders",
				"personal",
				"personas",
				"delivery",
				"webhooks",
				"mcp",
				"playbooks",
				"discord",
				"config",
			];
			let tabId = cleanPath === "/bot" ? "config" : cleanPath.slice(5); // "/bot/".length === 5
			if (!BOT_TABS.includes(tabId)) tabId = "config";
			window.currentBotId = botId;
			if (activeBotDisplay) {
				activeBotDisplay.textContent = botName || "ボット名";
			}
			updateSidebarBotBranding();

			loginOverlay.classList.remove("active");
			botSelectionOverlay.classList.remove("active");
			appContainer.classList.remove("hidden");

			switchTab(tabId);
			return;
		}

		// 未知のパス: ログイン済みなら Bot選択（ルート）へ、未ログインならログインへ。
		if (activeUserId) {
			navigateTo("/", false);
		} else {
			navigateTo("/login", false);
		}
	}

	menuItems.forEach((item) => {
		item.addEventListener("click", (e) => {
			e.preventDefault();
			const tabId = item.getAttribute("data-tab");
			navigateTo(`/bot/${tabId}`);
		});
	});

	// Usage Guide Link / Button Listeners
	const linkShowUsage = document.getElementById("link-show-usage");
	if (linkShowUsage) {
		linkShowUsage.addEventListener("click", (e) => {
			e.preventDefault();
			navigateTo("/usage");
		});
	}

	const btnUsageBackLogin = document.getElementById("btn-usage-back-login");
	if (btnUsageBackLogin) {
		btnUsageBackLogin.addEventListener("click", (e) => {
			e.preventDefault();
			navigateTo("/login");
		});
	}

	// Terms of Use Link / Button Listeners
	const linkShowTerms = document.getElementById("link-show-terms");
	if (linkShowTerms) {
		linkShowTerms.addEventListener("click", (e) => {
			const href = linkShowTerms.getAttribute("href");
			if (href && href.startsWith("/")) {
				e.preventDefault();
				navigateTo(href);
			}
		});
	}

	const btnTermsBackLogin = document.getElementById("btn-terms-back-login");
	if (btnTermsBackLogin) {
		btnTermsBackLogin.addEventListener("click", (e) => {
			e.preventDefault();
			navigateTo("/login");
		});
	}

	// Privacy Policy Link / Button Listeners
	const linkPrivacyPolicy = document.getElementById("link-privacy-policy");
	if (linkPrivacyPolicy) {
		linkPrivacyPolicy.addEventListener("click", (e) => {
			const href = linkPrivacyPolicy.getAttribute("href");
			if (href && href.startsWith("/")) {
				e.preventDefault();
				navigateTo(href);
			}
		});
	}

	const linkPrivacyPolicyUsage = document.getElementById(
		"link-privacy-policy-usage",
	);
	if (linkPrivacyPolicyUsage) {
		linkPrivacyPolicyUsage.addEventListener("click", (e) => {
			const href = linkPrivacyPolicyUsage.getAttribute("href");
			if (href && href.startsWith("/")) {
				e.preventDefault();
				navigateTo(href);
			}
		});
	}

	const btnPrivacyBackLogin = document.getElementById("btn-privacy-back-login");
	if (btnPrivacyBackLogin) {
		btnPrivacyBackLogin.addEventListener("click", (e) => {
			e.preventDefault();
			navigateTo("/login");
		});
	}

	// ==========================================
	// MODALS CONTROL
	// ==========================================
	function openModal(modal) {
		modal.classList.add("active");
	}

	function closeModal(modal) {
		modal.classList.remove("active");
	}

	document.querySelectorAll(".btn-close, .btn-close-modal").forEach((btn) => {
		btn.addEventListener("click", () => {
			closeModal(modalTask);
			closeModal(modalSchedule);
			closeModal(modalReceiptResult);
			closeModal(modalCredential);
			closeModal(modalCreateBot);
			[
				"modal-expense-plan",
				"modal-budget-settings",
				"modal-persona-edit",
				"modal-persona-preview",
				"modal-contact",
				"modal-webhook",
				"modal-audit",
				"modal-mcp-dashboard",
				"modal-int-calendars",
			].forEach((id) => {
				const m = document.getElementById(id);
				if (m) closeModal(m);
			});
			// 閉じる際にMCP管理ページの埋め込み(iframe)とリサイズリスナーを破棄する
			const mcpContainer = document.getElementById("mcp-dashboard-container");
			if (mcpContainer) teardownMcpDashboard(mcpContainer);
		});
	});

	// Open triggers
	document
		.getElementById("btn-new-task")
		.addEventListener("click", () => openModal(modalTask));
	document
		.getElementById("btn-new-schedule")
		.addEventListener("click", () => openModal(modalSchedule));
	document
		.getElementById("btn-new-credential")
		?.addEventListener("click", () => openModal(modalCredential));

	// ==========================================
	// API FETCH & CORE LOGIC
	// ==========================================

	// Execute calls based on visible view
	function loadDataForActiveTab() {
		if (activeTab === "dashboard") {
			fetchDashboardStats();
		} else if (activeTab === "tasks") {
			fetchTasksList();
		} else if (activeTab === "schedules") {
			fetchSchedulesList();
		} else if (activeTab === "expenses") {
			fetchExpensesList();
		} else if (activeTab === "reminders") {
			fetchRemindersList();
		} else if (activeTab === "personal") {
			fetchContextNote();
			fetchClipboardList();
			fetchContactsList();
		} else if (activeTab === "personas") {
			fetchPersonasList();
			fetchPersonaMarketplace();
			// 旧「Bot設定」から移設した Bot単位ペルソナ／推奨ペルソナのカードもこのタブで読み込む
			fetchBotAttributeConfig();
			fetchBotShares();
		} else if (activeTab === "discord") {
			fetchDiscordSettings();
		} else if (activeTab === "delivery") {
			fetchBriefingConfig();
			fetchReportConfigs();
		} else if (activeTab === "webhooks") {
			fetchWebhooksList();
			fetchWebhookDeliveries();
		} else if (activeTab === "mcp") {
			fetchMcpServersList();
		} else if (activeTab === "playbooks") {
			fetchPlaybooksList();
			fetchPlaybookSchedulesList();
			fetchPlaybookRunsList();
		} else if (activeTab === "config") {
			fetchConfigSettings();
		}
		// 統合管理 / 管理者用設定 は独立オーバーレイ（applyRoute で直接 fetch する）ため、ここでは扱わない。
	}

	// Tab switching inside login Overlay
	if (btnTabLogin && btnTabRegister) {
		btnTabLogin.addEventListener("click", () => {
			btnTabLogin.classList.add("active");
			btnTabRegister.classList.remove("active");
			loginTabContent.classList.add("active");
			registerTabContent.classList.remove("active");
			loginError.textContent = "";
		});

		btnTabRegister.addEventListener("click", () => {
			btnTabLogin.classList.remove("active");
			btnTabRegister.classList.add("active");
			loginTabContent.classList.remove("active");
			registerTabContent.classList.add("active");
			loginError.textContent = "";
		});
	}

	// Bot selection functions
	async function fetchBotList() {
		try {
			const res = await originalFetch("/api/bots");
			const data = await res.json();
			if (data.success) {
				renderBotList(data.bots);
			} else {
				alert("Bot一覧の取得に失敗しました: " + data.message);
			}
		} catch (err) {
			console.error(err);
			alert("Bot一覧の取得中にエラーが発生しました。");
		}
	}

	function renderBotList(bots) {
		botList.innerHTML = "";

		// ── 統計カードを更新 ──
		const statsEl = document.getElementById("home-stats");
		if (statsEl) {
			const total = bots.length;
			const online = bots.filter((b) => b.connected).length;
			const running = bots.filter((b) => b.running && !b.connected).length;
			const stopped = bots.filter((b) => !b.running).length;
			statsEl.innerHTML = `
        <div class="home-stat-card">
          <div class="home-stat-label">TOTAL BOTS</div>
          <div class="home-stat-value">${total}</div>
          <div class="home-stat-sub">登録済みアシスタント</div>
        </div>
        <div class="home-stat-card stat-online">
          <div class="home-stat-label">ONLINE</div>
          <div class="home-stat-value">${online}</div>
          <div class="home-stat-sub">Discord接続中</div>
        </div>
        <div class="home-stat-card${running > 0 ? " stat-warning" : ""}">
          <div class="home-stat-label">CONNECTING</div>
          <div class="home-stat-value">${running}</div>
          <div class="home-stat-sub">接続処理中</div>
        </div>
        <div class="home-stat-card">
          <div class="home-stat-label">STOPPED</div>
          <div class="home-stat-value">${stopped}</div>
          <div class="home-stat-sub">停止中</div>
        </div>
      `;
		}

		// ── Botカードを描画 ──
		bots.forEach((bot) => {
			const item = document.createElement("div");
			item.className = "bot-item-card";
			if (bot.id.startsWith("bot_default_") || bot.id === "system_default") {
				item.classList.add("default-bot");
			}

			const isCustomToken = !!bot.has_token;
			const isAssistant = bot.preset === "mcp_assistant";
			const accentColor = isAssistant
				? "#a78bfa"
				: isCustomToken
					? "#10b981"
					: "#3b82f6";
			const presetLabel =
				bot.preset_display_name ||
				(isAssistant ? "汎用モード" : "パーソナル秘書");

			// 稼働ステータス（バックエンドの botHealth と対応）
			let dotClass, statusLabel;
			if (bot.suspended) {
				dotClass = "dot-offline";
				statusLabel = "停止（管理者）";
			} else if (bot.connected) {
				// shared=独自トークン未設定。専用接続は持たず、通知等は共有(デフォルト)Bot経由で届く。
				dotClass = "dot-online";
				statusLabel = bot.shared ? "共有Botで稼働" : "稼働中";
			} else if (bot.running) {
				dotClass = "dot-connecting";
				statusLabel = "接続中…";
			} else {
				dotClass = "dot-offline";
				statusLabel = "停止中";
			}

			// アバター
			const avatarDiv = document.createElement("div");
			avatarDiv.className = "bot-avatar";
			if (bot.discord_avatar_url) {
				const img = document.createElement("img");
				img.src = bot.discord_avatar_url;
				img.alt = bot.discord_username || bot.name;
				img.onerror = () => {
					img.remove();
					avatarDiv.innerHTML = `<span class="material-symbols-outlined" style="font-size: 22px; color: ${accentColor};">robot_2</span>`;
				};
				avatarDiv.appendChild(img);
			} else {
				avatarDiv.innerHTML = `<span class="material-symbols-outlined" style="font-size: 22px; color: ${accentColor};">robot_2</span>`;
			}

			// カード上部（アバター + 名前）
			const cardTop = document.createElement("div");
			cardTop.className = "bot-card-top";
			const infoDiv = document.createElement("div");
			infoDiv.className = "bot-info";
			const displayName = bot.discord_username || bot.name;
			infoDiv.innerHTML = `
        <div class="bot-name" title="${escapeHtml(displayName)}">${escapeHtml(displayName)}</div>
        <div class="bot-status">${escapeHtml(presetLabel)}</div>
      `;
			cardTop.appendChild(avatarDiv);
			cardTop.appendChild(infoDiv);

			// カード下部（ステータス + アクション）
			const cardFooter = document.createElement("div");
			cardFooter.className = "bot-card-footer";

			const runStatus = document.createElement("div");
			runStatus.className = "bot-run-status";
			runStatus.innerHTML = `<span class="bot-run-dot ${dotClass}"></span>${statusLabel}`;

			const actionsDiv = document.createElement("div");
			actionsDiv.className = "bot-item-actions";

			// Discord同期ボタン
			const syncBtn = document.createElement("button");
			syncBtn.className = "btn-sync-discord";
			syncBtn.title = "Discordから名前・アイコンを同期";
			syncBtn.innerHTML = `<span class="material-symbols-outlined" style="font-size: 15px;">sync</span>`;
			syncBtn.addEventListener("click", async (e) => {
				e.stopPropagation();
				const icon = syncBtn.querySelector("span");
				if (icon) icon.style.animation = "spin 0.8s linear infinite";
				syncBtn.style.pointerEvents = "none";
				try {
					const res = await originalFetch("/api/bots/sync-discord", {
						method: "POST",
						headers: { "Content-Type": "application/json" },
						body: JSON.stringify({ botId: bot.id }),
					});
					const data = await res.json();
					if (data.success) {
						bot.discord_username = data.discord_username;
						bot.discord_avatar_url = data.discord_avatar_url;
						if (data.discord_avatar_url) {
							avatarDiv.innerHTML = "";
							const img = document.createElement("img");
							img.src = data.discord_avatar_url;
							img.alt = data.discord_username || bot.name;
							img.onerror = () => {
								img.remove();
								avatarDiv.innerHTML = `<span class="material-symbols-outlined" style="font-size: 22px; color: ${accentColor};">robot_2</span>`;
							};
							avatarDiv.appendChild(img);
						}
						infoDiv.querySelector(".bot-name").textContent =
							data.discord_username || bot.name;
					} else {
						alert("同期に失敗しました: " + data.message);
					}
				} catch (err) {
					alert("同期中にエラーが発生しました。");
				} finally {
					if (icon) icon.style.animation = "";
					syncBtn.style.pointerEvents = "";
				}
			});
			actionsDiv.appendChild(syncBtn);

			// 編集ボタン（デフォルトBot以外）
			if (!bot.id.startsWith("bot_default_") && bot.id !== "system_default") {
				const editBtn = document.createElement("button");
				editBtn.className = "btn-sync-discord";
				editBtn.title = "名前・アイコンを編集";
				editBtn.innerHTML = `<span class="material-symbols-outlined" style="font-size: 15px;">edit</span>`;
				editBtn.addEventListener("click", (e) => {
					e.stopPropagation();
					const profileModal = document.getElementById(
						"modal-edit-bot-profile",
					);
					document.getElementById("edit-bot-profile-id").value = bot.id;
					document.getElementById("edit-bot-profile-name").value =
						bot.discord_username || bot.name;
					document.getElementById("edit-bot-profile-avatar").value =
						bot.discord_avatar_url || "";
					if (profileModal) profileModal.classList.add("active");
				});
				actionsDiv.appendChild(editBtn);
			}

			// 削除ボタン（デフォルトBot以外）
			if (!bot.id.startsWith("bot_default_") && bot.id !== "system_default") {
				const delBtn = document.createElement("button");
				delBtn.className = "btn-delete-bot-inline";
				delBtn.title = "Botを削除";
				delBtn.innerHTML = `<span class="material-symbols-outlined" style="font-size: 15px;">delete</span>`;
				delBtn.addEventListener("click", async (e) => {
					e.stopPropagation();
					if (
						confirm(
							`本当にBot「${bot.name}」を削除しますか？\n紐づく経費やタスクデータも全て削除されます。`,
						)
					) {
						try {
							const res = await originalFetch("/api/bots", {
								method: "DELETE",
								headers: { "Content-Type": "application/json" },
								body: JSON.stringify({ botId: bot.id }),
							});
							const resData = await res.json();
							if (resData.success) {
								fetchBotList();
							} else {
								alert("削除に失敗しました: " + resData.message);
							}
						} catch (err) {
							alert("削除中にエラーが発生しました。");
						}
					}
				});
				actionsDiv.appendChild(delBtn);
			}

			cardFooter.appendChild(runStatus);
			cardFooter.appendChild(actionsDiv);

			item.appendChild(cardTop);
			item.appendChild(cardFooter);

			item.addEventListener("click", () => {
				selectBot(bot);
			});

			botList.appendChild(item);
		});
	}

	function selectBot(bot) {
		window.currentBotId = bot.id;
		localStorage.setItem("currentBotId", bot.id);
		localStorage.setItem("currentBotName", bot.discord_username || bot.name);
		localStorage.setItem("currentBotAvatar", bot.discord_avatar_url || "");
		localStorage.setItem("currentBotPreset", bot.preset || "secretary");
		if (activeBotDisplay) {
			activeBotDisplay.textContent = bot.discord_username || bot.name;
		}
		updateSidebarBotBranding();

		navigateTo("/bot/config");
	}

	// Bot Profile Edit Modal
	const modalEditBotProfile = document.getElementById("modal-edit-bot-profile");
	const editBotProfileForm = document.getElementById("edit-bot-profile-form");
	const btnCloseEditBotProfile = document.getElementById(
		"btn-close-edit-bot-profile",
	);

	if (btnCloseEditBotProfile && modalEditBotProfile) {
		btnCloseEditBotProfile.addEventListener("click", () =>
			modalEditBotProfile.classList.remove("active"),
		);
	}

	if (editBotProfileForm) {
		editBotProfileForm.addEventListener("submit", async (e) => {
			e.preventDefault();
			const botId = document.getElementById("edit-bot-profile-id").value;
			const name = document
				.getElementById("edit-bot-profile-name")
				.value.trim();
			const avatarUrl = document
				.getElementById("edit-bot-profile-avatar")
				.value.trim();

			try {
				const res = await originalFetch("/api/bots/profile", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify({ botId, name, avatarUrl: avatarUrl || null }),
				});
				const data = await res.json();
				if (data.success) {
					modalEditBotProfile.classList.remove("active");
					fetchBotList();
				} else {
					alert("更新に失敗しました: " + data.message);
				}
			} catch (err) {
				alert("通信エラーが発生しました。");
			}
		});
	}

	// Switch Bot handler
	if (btnSwitchBot) {
		btnSwitchBot.addEventListener("click", () => {
			navigateTo("/");
		});
	}
	const sidebarBtnBackBots = document.getElementById("sidebar-btn-back-bots");
	if (sidebarBtnBackBots) {
		sidebarBtnBackBots.addEventListener("click", () => {
			navigateTo("/");
		});
	}

	// 全体管理（Bot統合管理 / 管理者用設定）の入口・戻る配線。Bot個別画面の外で開く独立オーバーレイ。
	document
		.getElementById("btn-open-integrated")
		?.addEventListener("click", () => navigateTo("/integrated"));
	document
		.getElementById("btn-open-account")
		?.addEventListener("click", () => navigateTo("/account"));
	document
		.getElementById("btn-open-admin")
		?.addEventListener("click", () => navigateTo("/admin"));
	document
		.querySelectorAll("[data-management-back]")
		.forEach((btn) => btn.addEventListener("click", () => navigateTo("/")));

	// 統合管理／管理者設定のセクションはアコーディオン（同時に1つだけ展開）。
	// ネイティブの <details name> 排他制御に未対応なブラウザ向けの保険として、
	// あるセクションを開いたら同じ name の他セクションを閉じる。
	// tab-config（Bot単体の設定タブ）も同方式。name 無しのネスト details は対象外。
	[
		"integrated-overlay",
		"admin-overlay",
		"account-overlay",
		"tab-config",
	].forEach((overlayId) => {
		const root = document.getElementById(overlayId);
		if (!root) return;
		const items = root.querySelectorAll("details[name]");
		items.forEach((d) => {
			d.addEventListener("toggle", () => {
				if (!d.open) return;
				items.forEach((other) => {
					if (
						other !== d &&
						other.open &&
						other.getAttribute("name") === d.getAttribute("name")
					) {
						other.open = false;
					}
				});
			});
		});
	});

	/** プリセット表示名（管理ページから変更可能）を作成フォームへ反映する */
	async function refreshPresetOptions() {
		try {
			const res = await originalFetch("/api/bots/presets");
			const data = await res.json();
			if (!data.success || !Array.isArray(data.presets)) return;
			const select = document.getElementById("new-bot-preset");
			if (!select) return;
			data.presets.forEach((p) => {
				const option = select.querySelector(`option[value="${p.id}"]`);
				if (option) option.textContent = p.displayName;
			});
		} catch (e) {
			/* 表示名はHTMLの既定値で続行 */
		}
	}

	if (btnShowAddBot) {
		btnShowAddBot.addEventListener("click", () => {
			refreshPresetOptions();
			openModal(modalCreateBot);
		});
	}

	if (createBotForm) {
		createBotForm.addEventListener("submit", async (e) => {
			e.preventDefault();
			const name = document.getElementById("new-bot-name").value.trim();
			const presetSelect = document.getElementById("new-bot-preset");
			const preset = presetSelect ? presetSelect.value : "secretary";

			try {
				const res = await originalFetch("/api/bots", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify({ name, preset }),
				});
				const data = await res.json();
				if (data.success) {
					closeModal(modalCreateBot);
					createBotForm.reset();
					if (preset === "mcp_assistant" && data.message) {
						alert(data.message);
					}
					fetchBotList();
				} else {
					alert("Bot作成に失敗しました: " + data.message);
				}
			} catch (err) {
				alert("Bot作成中にエラーが発生しました。");
			}
		});
	}

	if (btnBotLogout) {
		btnBotLogout.addEventListener("click", async () => {
			try {
				await originalFetch("/api/logout", { method: "POST" });
			} catch (e) {}
			localStorage.removeItem("currentBotId");
			localStorage.removeItem("currentBotName");
			window.currentBotId = null;
			activeUserId = "";
			navigateTo("/login");
		});
	}

	function updatePrivacyPolicyLinks(url) {
		const linkLogin = document.getElementById("link-privacy-policy");
		const linkUsage = document.getElementById("link-privacy-policy-usage");
		if (url && url.trim() !== "") {
			const isInternal = url.startsWith("/");
			[linkLogin, linkUsage].forEach((link) => {
				if (link) {
					link.href = url;
					if (isInternal) {
						link.removeAttribute("target");
						link.removeAttribute("rel");
					} else {
						link.setAttribute("target", "_blank");
						link.setAttribute("rel", "noopener");
					}
					link.classList.remove("hidden");
				}
			});
		} else {
			[linkLogin, linkUsage].forEach((link) => {
				if (link) {
					link.removeAttribute("href");
					link.classList.add("hidden");
				}
			});
		}
	}

	function updateTermsLinks(url) {
		const linkLogin = document.getElementById("link-show-terms");
		if (url && url.trim() !== "") {
			const isInternal = url.startsWith("/");
			if (linkLogin) {
				linkLogin.href = url;
				if (isInternal) {
					linkLogin.removeAttribute("target");
					linkLogin.removeAttribute("rel");
				} else {
					linkLogin.setAttribute("target", "_blank");
					linkLogin.setAttribute("rel", "noopener");
				}
				linkLogin.classList.remove("hidden");
			}
		} else {
			if (linkLogin) {
				linkLogin.removeAttribute("href");
				linkLogin.classList.add("hidden");
			}
		}
	}

	async function checkSetupStatus() {
		try {
			const res = await originalFetch("/api/setup/status");
			const data = await res.json();

			if (data.privacyPolicyUrl !== undefined) {
				updatePrivacyPolicyLinks(data.privacyPolicyUrl);
			}

			const loginTabContent = document.getElementById("login-tab-content");
			const registerTabContent = document.getElementById(
				"register-tab-content",
			);
			const setupTabContent = document.getElementById("setup-tab-content");
			const botSetupTabContent = document.getElementById(
				"bot-setup-tab-content",
			);
			const loginTabs = document.querySelector(".login-tabs");

			if (data.needSetup) {
				if (loginTabs) loginTabs.style.display = "none";
				if (loginTabContent) loginTabContent.classList.remove("active");
				if (registerTabContent) registerTabContent.classList.remove("active");
				if (botSetupTabContent) botSetupTabContent.classList.remove("active");
				if (setupTabContent) setupTabContent.classList.add("active");
				return true;
			} else {
				if (loginTabs) loginTabs.style.display = "";
				if (setupTabContent) setupTabContent.classList.remove("active");
				return false;
			}
		} catch (err) {
			console.error("Failed to check setup status:", err);
			return false;
		}
	}

	function showDefaultBotSetupScreen() {
		loginOverlay.classList.add("active");
		botSelectionOverlay.classList.remove("active");
		appContainer.classList.add("hidden");

		const loginTabs = document.querySelector(".login-tabs");
		if (loginTabs) loginTabs.style.display = "none";

		const loginTabContent = document.getElementById("login-tab-content");
		const registerTabContent = document.getElementById("register-tab-content");
		const setupTabContent = document.getElementById("setup-tab-content");
		const botSetupTabContent = document.getElementById("bot-setup-tab-content");

		if (loginTabContent) loginTabContent.classList.remove("active");
		if (registerTabContent) registerTabContent.classList.remove("active");
		if (setupTabContent) setupTabContent.classList.remove("active");
		if (botSetupTabContent) botSetupTabContent.classList.add("active");
	}

	// App Session Initialization
	async function initAppSession() {
		try {
			const res = await originalFetch("/api/me");
			const data = await res.json();
			if (data.success) {
				if (data.privacyPolicyUrl !== undefined) {
					updatePrivacyPolicyLinks(data.privacyPolicyUrl);
				}
				if (data.termsUrl !== undefined) {
					updateTermsLinks(data.termsUrl);
				}

				activeUserId = data.user.discordId;
				activeUserRole = data.user.role || "user";
				document.getElementById("current-user-display").textContent =
					`${data.user.username} (${activeUserId})`;
				const homeUsernameEl = document.getElementById("home-username-display");
				if (homeUsernameEl) homeUsernameEl.textContent = data.user.username;

				// Admin 入口（Bot選択画面の「管理者用設定」ボタン）の表示/非表示制御
				const adminEntryBtn = document.getElementById("btn-open-admin");
				if (adminEntryBtn) {
					adminEntryBtn.style.display =
						activeUserRole === "admin" ? "inline-flex" : "none";
				}

				// 管理者の場合、デフォルトBotが設定されているか確認する
				if (activeUserRole === "admin") {
					try {
						const botsRes = await originalFetch("/api/bots");
						const botsData = await botsRes.json();
						const systemDefaultBot =
							botsData.success &&
							botsData.bots.find((b) => b.id === "system_default");
						// バックエンドは機密のため生トークンを返さず has_token（真偽）のみを返す。
						const hasDefaultToken =
							systemDefaultBot && systemDefaultBot.has_token;
						if (!hasDefaultToken) {
							showDefaultBotSetupScreen();
							return;
						}
					} catch (err) {
						console.error("Default bot check error:", err);
					}
				}

				// If already on a specific deep-link path, navigate there directly
				const currentPath = window.location.pathname;
				if (
					currentPath !== "/" &&
					currentPath !== "/index.html" &&
					currentPath !== "/login"
				) {
					applyRoute(currentPath);
				} else {
					// ルート/ログインからの初期表示は Bot選択画面（"/"）へ。
					navigateTo("/", false);
				}
			} else {
				activeUserId = "";
				const needSetup = await checkSetupStatus();
				if (!needSetup) {
					const btnTabLogin = document.getElementById("btn-tab-login");
					const loginTabContent = document.getElementById("login-tab-content");
					if (btnTabLogin) btnTabLogin.classList.add("active");
					if (loginTabContent) loginTabContent.classList.add("active");
				}
				navigateTo("/login", false);
			}
		} catch (err) {
			activeUserId = "";
			await checkSetupStatus();
			navigateTo("/login", false);
		}
	}

	// A. AUTHENTICATION & LOGIN FLOW
	loginForm.addEventListener("submit", async (e) => {
		e.preventDefault();
		loginError.textContent = "";
		const discordId = document.getElementById("login-discord-id").value.trim();
		const password = document.getElementById("login-password").value;

		try {
			const res = await originalFetch("/api/login", {
				method: "POST",
				headers: { "Content-Type": "application/json" },
				body: JSON.stringify({ discordId, password }),
			});
			const data = await res.json();

			if (data.success) {
				await initAppSession();
			} else {
				loginError.textContent = data.message;
			}
		} catch (err) {
			loginError.textContent = "サーバー接続に失敗しました。";
		}
	});

	// INITIAL SETUP FLOW (Step 1: Admin Register)
	const setupForm = document.getElementById("setup-form");
	if (setupForm) {
		setupForm.addEventListener("submit", async (e) => {
			e.preventDefault();
			loginError.textContent = "";
			const discordId = document
				.getElementById("setup-discord-id")
				.value.trim();
			const username = document.getElementById("setup-username").value.trim();
			const password = document.getElementById("setup-password").value;
			const geminiApiKey = document
				.getElementById("setup-gemini-key")
				.value.trim();

			try {
				const res = await originalFetch("/api/setup", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify({ discordId, username, password, geminiApiKey }),
				});
				const data = await res.json();

				if (data.success) {
					alert(
						"管理者アカウントの登録が完了しました。続いてデフォルトBotを設定してください。",
					);
					await initAppSession();
				} else {
					loginError.textContent = data.message;
				}
			} catch (err) {
				loginError.textContent = "サーバー接続に失敗しました。";
			}
		});
	}

	// INITIAL SETUP FLOW (Step 2: Default Bot Token Setup)
	const botSetupForm = document.getElementById("bot-setup-form");
	if (botSetupForm) {
		botSetupForm.addEventListener("submit", async (e) => {
			e.preventDefault();
			loginError.textContent = "";
			const token = document.getElementById("bot-setup-token").value.trim();

			try {
				const res = await originalFetch("/api/admin/default-bot/token", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify({ token }),
				});
				const data = await res.json();

				if (data.success) {
					alert("デフォルトBotのセットアップが完了しました！");
					window.currentBotId = "system_default";
					localStorage.setItem("currentBotId", "system_default");
					localStorage.setItem("currentBotName", "システムデフォルト");
					localStorage.setItem("currentBotAvatar", "");
					updateSidebarBotBranding();

					loginOverlay.classList.remove("active");
					navigateTo("/bot/dashboard");
				} else {
					loginError.textContent = data.message;
				}
			} catch (err) {
				loginError.textContent = "サーバー接続に失敗しました。";
			}
		});
	}

	// ACCOUNT REGISTRATION FLOW
	registerForm.addEventListener("submit", async (e) => {
		e.preventDefault();
		loginError.textContent = "";
		const discordId = document.getElementById("reg-discord-id").value.trim();
		const username = document.getElementById("reg-username").value.trim();
		const password = document.getElementById("reg-password").value;
		const inviteCode = document.getElementById("reg-invite-code").value.trim();
		const geminiApiKey = document.getElementById("reg-gemini-key").value.trim();

		try {
			const res = await originalFetch("/api/register", {
				method: "POST",
				headers: { "Content-Type": "application/json" },
				body: JSON.stringify({
					discordId,
					username,
					password,
					inviteCode,
					geminiApiKey,
				}),
			});
			const data = await res.json();

			if (data.success && data.pending) {
				// DMチャレンジ: 確認コード入力ステップへ遷移（G1）
				pendingRegisterDiscordId = discordId;
				registerForm.style.display = "none";
				registerVerifyForm.style.display = "";
				document.getElementById("reg-verify-code").value = "";
				document.getElementById("reg-verify-code").focus();
			} else if (data.success) {
				alert("アカウントが作成されました！ログインを行ってください。");
				btnTabLogin.click();
				document.getElementById("login-discord-id").value = discordId;
			} else {
				loginError.textContent = data.message;
			}
		} catch (err) {
			loginError.textContent = "サーバー接続に失敗しました。";
		}
	});

	// 登録 確認コードの検証（DMチャレンジ）
	let pendingRegisterDiscordId = "";
	const registerVerifyForm = document.getElementById("register-verify-form");
	function resetRegisterToForm() {
		registerVerifyForm.style.display = "none";
		registerForm.style.display = "";
		pendingRegisterDiscordId = "";
	}
	document.getElementById("reg-verify-back").addEventListener("click", () => {
		loginError.textContent = "";
		resetRegisterToForm();
	});
	registerVerifyForm.addEventListener("submit", async (e) => {
		e.preventDefault();
		loginError.textContent = "";
		const code = document.getElementById("reg-verify-code").value.trim();
		try {
			const res = await originalFetch("/api/register/verify", {
				method: "POST",
				headers: { "Content-Type": "application/json" },
				body: JSON.stringify({ discordId: pendingRegisterDiscordId, code }),
			});
			const data = await res.json();
			if (data.success) {
				const did = pendingRegisterDiscordId;
				resetRegisterToForm();
				registerForm.reset();
				alert("アカウントが作成されました！ログインを行ってください。");
				btnTabLogin.click();
				document.getElementById("login-discord-id").value = did;
			} else {
				loginError.textContent = data.message;
			}
		} catch (err) {
			loginError.textContent = "サーバー接続に失敗しました。";
		}
	});

	// B. LOGOUT FLOW
	btnLogout.addEventListener("click", async () => {
		try {
			await originalFetch("/api/logout", { method: "POST" });
		} catch (e) {}

		localStorage.removeItem("currentBotId");
		localStorage.removeItem("currentBotName");
		window.currentBotId = null;
		activeUserId = "";
		navigateTo("/login");
		loginError.textContent = "ログアウトしました。";
	});

	// D. DASHBOARD STATS POLL
	async function fetchDashboardStats() {
		// 汎用モードでは秘書統計(/api/status, /api/expenses)はゼロになるため、
		// API使用量ダッシュボードを描画して終了する。
		applyDashboardMode();
		if (currentBotPreset() === "mcp_assistant") {
			return fetchAssistantUsageDashboard();
		}
		try {
			const statusRes = await fetch("/api/status");
			const statusData = await statusRes.json();

			const expenseRes = await fetch("/api/expenses");
			const expenseData = await expenseRes.json();

			if (statusData.success && expenseData.success) {
				pendingTasksCount = statusData.stats.pendingTasks;
				totalExpensesVal = expenseData.total;

				// Populate Stats DOM
				statPendingTasks.textContent = pendingTasksCount;
				statUpcomingSchedules.textContent = statusData.stats.schedules;
				statExpensesTotal.textContent = `¥${totalExpensesVal.toLocaleString()}`;

				// Speech bubble update
				updateYuukaSpeechBubble();

				// Render Priority Mini Bar Chart inside Card 1 (Tasks)
				const priorityChart = document.getElementById(
					"tasks-priority-bar-chart",
				);
				if (priorityChart) {
					priorityChart.replaceChildren();
					const priorities = statusData.stats.pendingPriorities || {
						0: 0,
						1: 0,
						2: 0,
					};
					const maxPrioCount = Math.max(
						priorities[0],
						priorities[1],
						priorities[2],
						1,
					);

					const colors =
						currentTheme() === "blue-archive"
							? ["#B8E8F8", "#51C8E8", "#02D3FB"] // BA: light/mid/full cyan
							: ["#4f545c", "#fbbf24", "#da373c"]; // Low: Muted Grey, Medium: Warning Yellow, High: Discord Red
					const labels = ["低", "中", "高"];

					[0, 1, 2].forEach((p) => {
						const barContainer = document.createElement("div");
						barContainer.style.display = "flex";
						barContainer.style.flexDirection = "column";
						barContainer.style.alignItems = "center";
						barContainer.style.flex = "1";
						barContainer.style.height = "100%";
						barContainer.style.justifyContent = "flex-end";
						barContainer.title = `${labels[p]}優先度: ${priorities[p]}件`;

						const bar = document.createElement("div");
						const pct = (priorities[p] / maxPrioCount) * 100;
						bar.style.height = `${Math.max(pct, 10)}%`;
						bar.style.width = "100%";
						bar.style.backgroundColor = colors[p];
						bar.style.borderRadius = "var(--radius)";
						bar.style.transition = "height 0.3s ease";

						const countText = document.createElement("span");
						countText.style.fontSize = "0.55rem";
						countText.style.fontFamily = "var(--font-family-mono)";
						countText.style.color = "var(--color-zinc-muted)";
						countText.style.marginBottom = "2px";
						countText.textContent = priorities[p];

						barContainer.appendChild(countText);
						barContainer.appendChild(bar);
						priorityChart.appendChild(barContainer);
					});
				}

				// Render Schedules Sparkline (real line over next 5 days)
				const schedulesPath = document.getElementById(
					"schedules-sparkline-path",
				);
				if (schedulesPath) {
					const trend = statusData.stats.scheduleTrend || [0, 0, 0, 0, 0];
					const maxVal = Math.max(...trend, 2);

					const points = trend.map((val, idx) => {
						const x = idx * 25;
						const y = 19 - (val / maxVal) * 18;
						return `${x},${y}`;
					});
					const pathStr = `M ${points.map((p, idx) => `${idx === 0 ? "" : "L "} ${p}`).join(" ")}`;
					schedulesPath.setAttribute("d", pathStr);
				}

				// Render Expenses Sparkline (real line over past 5 days)
				const expensesPath = document.getElementById("expenses-sparkline-path");
				if (expensesPath) {
					const trend = statusData.stats.expenseTrend || [0, 0, 0, 0, 0];
					const maxVal = Math.max(...trend, 5000);

					const points = trend.map((val, idx) => {
						const x = idx * 25;
						const y = 19 - (val / maxVal) * 18;
						return `${x},${y}`;
					});
					const pathStr = `M ${points.map((p, idx) => `${idx === 0 ? "" : "L "} ${p}`).join(" ")}`;
					expensesPath.setAttribute("d", pathStr);
				}

				// Render Donut Chart
				renderDonutChart(expenseData.breakdown, expenseData.total);

				// Fetch Urgent items (pending tasks & today's schedules combined)
				renderUrgentDashboardList(statusData.stats.pendingTasks);

				// Extra Crypto Ticker Analytics
				const maxExp =
					expenseData.expenses && expenseData.expenses.length > 0
						? Math.max(...expenseData.expenses.map((e) => e.amount))
						: 0;
				document.getElementById("dashboard-highest-expense").textContent =
					`¥${maxExp.toLocaleString()}`;

				const maxCat =
					expenseData.breakdown && expenseData.breakdown.length > 0
						? expenseData.breakdown[0].category
						: "なし";
				document.getElementById("dashboard-highest-category").textContent =
					maxCat;

				const avg =
					expenseData.expenses && expenseData.expenses.length > 0
						? Math.round(expenseData.total / 30)
						: 0;
				document.getElementById("dashboard-average-expense").textContent =
					`¥${avg.toLocaleString()}`;

				// Render interactive Price Trend Line Chart
				renderPriceTrendChart(expenseData.expenses);
			}
		} catch (err) {
			console.error("ダッシュボード情報の更新エラー:", err);
			yuukaBubbleText.textContent =
				"ダッシュボード情報の取得中にエラーが発生しました。サーバーの接続状況を確認してください。";
		}
	}

	// ── 汎用モード: API使用量ダッシュボード ───────────────────────────────────────

	function setUsageText(id, value) {
		const el = document.getElementById(id);
		if (el) el.textContent = value;
	}

	/** 'YYYY-MM-DD' を 'M/D' に整形する（先頭ゼロなし） */
	function formatMonthDay(dateStr) {
		const parts = String(dateStr).split("-");
		if (parts.length < 3) return dateStr;
		return `${parseInt(parts[1], 10)}/${parseInt(parts[2], 10)}`;
	}

	async function fetchAssistantUsageDashboard() {
		try {
			const botId =
				window.currentBotId ||
				localStorage.getItem("currentBotId") ||
				"system_default";
			// 注: /api/bots/* は fetch override の botId 自動付与対象外のため、明示的に付与する
			const res = await originalFetch(
				`/api/bots/usage?botId=${encodeURIComponent(botId)}&days=14`,
			);
			const data = await res.json();
			if (!data.success) {
				if (yuukaBubbleText)
					yuukaBubbleText.textContent = "API使用量の取得に失敗しました。";
				return;
			}
			renderUsageDashboard(data);
			updateAssistantSpeechBubble(data);
		} catch (err) {
			console.error("API使用量の取得エラー:", err);
			if (yuukaBubbleText) {
				yuukaBubbleText.textContent =
					"API使用量の取得中にエラーが発生しました。サーバーの接続状況を確認してください。";
			}
		}
	}

	function renderUsageDashboard(data) {
		const series = Array.isArray(data.series) ? data.series : [];
		const totals = data.totals || { requests: 0, responses: 0 };
		const days = data.days || series.length || 14;

		const today = series.length ? series[series.length - 1].requests : 0;
		const totalReq = totals.requests || 0;
		const totalRes = totals.responses || 0;
		const avg = series.length ? Math.round(totalReq / series.length) : 0;

		let peak = 0;
		let peakDate = "";
		series.forEach((s) => {
			if (s.requests > peak) {
				peak = s.requests;
				peakDate = s.date;
			}
		});

		setUsageText("usage-stat-today", today.toLocaleString());
		setUsageText("usage-stat-total", totalReq.toLocaleString());
		setUsageText("usage-stat-avg", avg.toLocaleString());
		setUsageText("usage-stat-peak", peak.toLocaleString());
		setUsageText(
			"usage-stat-peak-date",
			peakDate ? `${formatMonthDay(peakDate)} が最多` : "データなし",
		);
		setUsageText("usage-stat-period-badge", `${days} DAYS`);
		setUsageText("usage-period-label", `${days}日`);
		setUsageText("usage-total-responses", totalRes.toLocaleString());
		setUsageText(
			"usage-response-rate",
			totalReq > 0 ? `${Math.round((totalRes / totalReq) * 100)}%` : "—",
		);

		renderUsageChart(series);
		renderUsageAxes(series);
		renderRateLimits(data.rate_limits);
	}

	/** API使用量推移のSVGチャートを描画する（preserveAspectRatio=none + 非スケール線幅で全幅表示） */
	function renderUsageChart(series) {
		const svg = document.getElementById("usage-trend-chart");
		if (!svg) return;

		const W = 400;
		const H = 150;
		const padTop = 12;
		const padBottom = 8;
		const innerH = H - padTop - padBottom;
		const n = series.length;

		if (n === 0) {
			svg.replaceChildren();
			return;
		}

		const maxVal = Math.max(
			1,
			...series.map((s) => Math.max(s.requests, s.responses)),
		);
		const xAt = (i) => (n <= 1 ? W / 2 : (i / (n - 1)) * W);
		const yAt = (v) => padTop + innerH - (v / maxVal) * innerH;
		const toLine = (pts) =>
			pts
				.map(
					(p, i) =>
						`${i === 0 ? "M" : "L"}${p[0].toFixed(1)},${p[1].toFixed(1)}`,
				)
				.join(" ");

		const reqPts = series.map((s, i) => [xAt(i), yAt(s.requests)]);
		const resPts = series.map((s, i) => [xAt(i), yAt(s.responses)]);
		const reqLine = toLine(reqPts);
		const resLine = toLine(resPts);
		const baseY = (padTop + innerH).toFixed(1);
		const reqArea = `${reqLine} L${xAt(n - 1).toFixed(1)},${baseY} L${xAt(0).toFixed(1)},${baseY} Z`;

		const gridLines = [0.25, 0.5, 0.75]
			.map((f) => {
				const y = (padTop + innerH - f * innerH).toFixed(1);
				return `<line class="usage-chart-grid-line" x1="0" y1="${y}" x2="${W}" y2="${y}" />`;
			})
			.join("");

		// 注: 座標は信頼できる数値のみ（series はサーバ集計）。文字列の混入が無いため innerHTML を使用。
		svg.innerHTML =
			gridLines +
			`<path class="usage-chart-req-area" d="${reqArea}" />` +
			`<path class="usage-chart-res-line" d="${resLine}" vector-effect="non-scaling-stroke" />` +
			`<path class="usage-chart-req-line" d="${reqLine}" vector-effect="non-scaling-stroke" />`;
	}

	/** チャートのX軸日付ラベルとY軸目盛を描画する */
	function renderUsageAxes(series) {
		const n = series.length;
		const maxVal = Math.max(
			1,
			...series.map((s) => Math.max(s.requests, s.responses)),
		);

		const yaxis = document.getElementById("usage-chart-yaxis");
		if (yaxis) {
			yaxis.replaceChildren();
			[maxVal, Math.round((maxVal * 2) / 3), Math.round(maxVal / 3), 0].forEach(
				(v) => {
					const sp = document.createElement("span");
					sp.textContent = v;
					yaxis.appendChild(sp);
				},
			);
		}

		const xaxis = document.getElementById("usage-chart-xaxis");
		if (xaxis) {
			xaxis.replaceChildren();
			if (n > 0) {
				const labelCount = Math.min(5, n);
				const idxs = [];
				for (let k = 0; k < labelCount; k++) {
					idxs.push(Math.round((k / (labelCount - 1 || 1)) * (n - 1)));
				}
				[...new Set(idxs)].forEach((i) => {
					const sp = document.createElement("span");
					sp.textContent = formatMonthDay(series[i].date);
					xaxis.appendChild(sp);
				});
			}
		}
	}

	function renderRateLimits(rl) {
		const grid = document.getElementById("usage-ratelimit-grid");
		if (!grid) return;
		grid.replaceChildren();

		if (!rl) {
			const note = document.createElement("div");
			note.className = "usage-empty-note";
			note.textContent = "レート制限の設定情報を取得できませんでした。";
			grid.appendChild(note);
			return;
		}

		const items = [
			{ label: "ユーザー1人 / 分", value: rl.userPerMinute },
			{ label: "ユーザー1人 / 日", value: rl.userPerDay },
			{ label: "サーバー1つ / 日", value: rl.guildPerDay },
		];
		items.forEach((it) => {
			const card = document.createElement("div");
			card.className = "usage-ratelimit-item";

			const val = document.createElement("div");
			val.className = "usage-rl-value";
			val.textContent =
				it.value === undefined || it.value === null
					? "—"
					: it.value.toLocaleString();

			const lbl = document.createElement("div");
			lbl.className = "usage-rl-label";
			lbl.textContent = it.label;

			card.appendChild(val);
			card.appendChild(lbl);
			grid.appendChild(card);
		});
	}

	function updateAssistantSpeechBubble(data) {
		if (!yuukaBubbleText) return;
		const series = Array.isArray(data.series) ? data.series : [];
		const today = series.length ? series[series.length - 1].requests : 0;
		const total = (data.totals && data.totals.requests) || 0;
		const days = data.days || series.length || 14;
		if (total === 0) {
			yuukaBubbleText.textContent =
				"まだAPIの利用がありません。Discordサーバーでこのアシスタントにメンションして話しかけてみましょう。";
		} else {
			yuukaBubbleText.textContent = `直近${days}日間で ${total.toLocaleString()} 件のリクエストを処理しました（本日 ${today.toLocaleString()} 件）。下のチャートでAPI使用量の推移を確認できます。`;
		}
	}

	function updateYuukaSpeechBubble() {
		let text = "";
		if (totalExpensesVal > 30000) {
			text = `今月の支出が ¥${totalExpensesVal.toLocaleString()} に達しています。予算を確認してみましょう。`;
		} else if (pendingTasksCount > 5) {
			text = `未完了タスクが ${pendingTasksCount} 件あります。優先度の高いものから片付けていきましょう。`;
		} else {
			text =
				"お疲れ様です。タスク・予定・家計の管理をサポートします。何かあればDiscordでお声がけください。";
		}
		yuukaBubbleText.textContent = text;
	}

	function renderDonutChart(breakdown, total) {
		dashboardCategoryLegend.replaceChildren();

		if (!breakdown || breakdown.length === 0 || total === 0) {
			donutSegment.setAttribute("stroke-dasharray", "0 251.2");
			chartCenterPercentage.textContent = "0%";

			const empty = document.createElement("div");
			empty.className = "legend-item";
			empty.textContent = "今月のデータはありません。";
			dashboardCategoryLegend.appendChild(empty);
			return;
		}

		const entertainment = breakdown.find((c) => c.category === "娯楽");
		const entPct = entertainment
			? Math.round((entertainment.total / total) * 100)
			: 0;

		const strokeDash = (entPct * 251.2) / 100;
		donutSegment.setAttribute("stroke-dasharray", `${strokeDash} 251.2`);
		chartCenterPercentage.textContent = `${entPct}%`;

		// 凡例ドットの配色（BAテーマ時はゲーム配色に切り替え）
		const colors =
			currentTheme() === "blue-archive"
				? {
						食費: "#02D3FB", // BAシアン
						日用品: "#A3BAFF", // ラベンダー
						交通費: "#FB90A4", // MomoTalk ピンク
						娯楽: "#FFD966", // ウォームイエロー
						その他: "#7A9BB0", // ミュートグレー
					}
				: {
						食費: "#5865F2", // Discord Blurple
						日用品: "#248046", // Success green
						交通費: "#fbbf24", // Warning yellow
						娯楽: "#f472b6", // Pink
						その他: "#4f545c", // Grey
					};

		breakdown.slice(0, 4).forEach((cat) => {
			const pct = Math.round((cat.total / total) * 100);
			const dotColor = colors[cat.category] || "#a78bfa";

			const item = document.createElement("div");
			item.className = "legend-item";

			const colorBox = document.createElement("span");
			colorBox.className = "legend-color";
			colorBox.style.backgroundColor = dotColor;

			const label = document.createElement("span");
			label.textContent = `${cat.category}: ¥${cat.total.toLocaleString()} (${pct}%)`;

			item.appendChild(colorBox);
			item.appendChild(label);
			dashboardCategoryLegend.appendChild(item);
		});
	}

	function renderPriceTrendChart(expenses) {
		const trendLinePath = document.getElementById("trend-line-path");
		const trendAreaPath = document.getElementById("trend-area-path");
		const trendChartSvg = document.getElementById("dashboard-trend-chart");

		const circles = trendChartSvg.querySelectorAll("circle");
		circles.forEach((c) => c.remove());

		const dateLabels = [];
		const dateStrings = [];

		for (let i = 5; i >= 0; i--) {
			const d = new Date();
			d.setDate(d.getDate() - i);
			dateStrings.push(d.toISOString().slice(0, 10));
			dateLabels.push(`${d.getMonth() + 1}/${d.getDate()}`);
		}

		const xAxisContainer = trendChartSvg.nextElementSibling;
		xAxisContainer.replaceChildren();
		dateLabels.forEach((label, idx) => {
			const span = document.createElement("span");
			span.textContent =
				idx === 0 ? "5日前" : idx === 4 ? "昨日" : idx === 5 ? "今日" : label;
			xAxisContainer.appendChild(span);
		});

		const dailyTotals = dateStrings.map((date) => {
			if (!expenses) return 0;
			return expenses
				.filter((e) => e.date === date)
				.reduce((sum, e) => sum + e.amount, 0);
		});

		const maxVal = Math.max(...dailyTotals, 10000);

		const points = dailyTotals.map((val, idx) => {
			const x = idx * 80;
			const y = 130 - (val / maxVal) * 100;
			return { x, y, amount: val };
		});

		const linePathStr = points
			.map((p, idx) => `${idx === 0 ? "M" : "L"} ${p.x},${p.y}`)
			.join(" ");
		trendLinePath.setAttribute("d", linePathStr);

		const areaPathStr = `M 0,130 ${points.map((p) => `L ${p.x},${p.y}`).join(" ")} L 400,130 Z`;
		trendAreaPath.setAttribute("d", areaPathStr);

		points.forEach((p) => {
			const circle = document.createElementNS(
				"http://www.w3.org/2000/svg",
				"circle",
			);
			circle.setAttribute("cx", p.x);
			circle.setAttribute("cy", p.y);
			circle.setAttribute("r", "4.5");
			circle.setAttribute("fill", "var(--color-primary)");
			circle.setAttribute("stroke", "var(--bg-primary)");
			circle.setAttribute("stroke-width", "1.5");
			circle.style.cursor = "pointer";
			circle.style.transition = "r 0.15s ease, fill 0.15s ease";

			circle.addEventListener("mouseenter", () => {
				circle.setAttribute("r", "7.5");
				circle.setAttribute("fill", "var(--color-white)");
				yuukaBubbleText.textContent = `${p.x === 400 ? "今日" : p.x === 320 ? "昨日" : "この日"}の出費額は ¥${p.amount.toLocaleString()} です。`;
			});

			circle.addEventListener("mouseleave", () => {
				circle.setAttribute("r", "4.5");
				circle.setAttribute("fill", "var(--discord-blurple)");
				updateYuukaSpeechBubble();
			});

			trendChartSvg.appendChild(circle);
		});
	}

	async function renderUrgentDashboardList(pendingCount) {
		dashboardUrgentList.replaceChildren();

		try {
			const resSched = await fetch("/api/schedules?days=1");
			const dataSched = await resSched.json();

			const resTasks = await fetch("/api/tasks?status=pending");
			const dataTasks = await resTasks.json();

			let count = 0;

			if (dataSched.success && dataSched.schedules.length > 0) {
				dataSched.schedules.slice(0, 2).forEach((sched) => {
					const item = document.createElement("div");
					item.className = "urgent-item";

					const iconSpan = document.createElement("span");
					iconSpan.className = "material-symbols-outlined icon-small";
					iconSpan.style.marginRight = "6px";
					iconSpan.textContent = "calendar_today";

					const title = document.createElement("span");
					title.textContent = ` [今日の予定] ${sched.title}`;
					title.style.fontWeight = "bold";

					const badge = document.createElement("span");
					badge.className = "urgent-badge badge-urgent";
					badge.textContent = sched.start_at.slice(11, 16);

					item.appendChild(iconSpan);
					item.appendChild(title);
					item.appendChild(badge);
					dashboardUrgentList.appendChild(item);
					count++;
				});
			}

			if (dataTasks.success && dataTasks.tasks.length > 0) {
				dataTasks.tasks.slice(0, 3).forEach((task) => {
					const item = document.createElement("div");
					item.className = "urgent-item";

					const iconSpan = document.createElement("span");
					iconSpan.className = "material-symbols-outlined icon-small";
					iconSpan.style.marginRight = "6px";
					iconSpan.textContent = "checklist";

					const title = document.createElement("span");
					title.textContent = ` [未消化タスク] ${task.title}`;

					const badge = document.createElement("span");
					badge.className = "urgent-badge badge-normal";
					badge.textContent =
						task.priority === "high" ? "優先: 高" : "優先: 普通";

					item.appendChild(iconSpan);
					item.appendChild(title);
					item.appendChild(badge);
					dashboardUrgentList.appendChild(item);
					count++;
				});
			}

			if (count === 0) {
				const item = document.createElement("div");
				item.className = "urgent-item";
				item.textContent =
					"今日の急ぎのタスクや予定はありません！素晴らしい計画性ですね！";
				dashboardUrgentList.appendChild(item);
			}
		} catch (e) {
			console.error(e);
		}
	}

	// E. TASKS VIEW LOGIC (Fetch & CRUD)
	async function fetchTasksList(filter = "all") {
		tasksList.replaceChildren();

		try {
			const res = await fetch(`/api/tasks?status=${filter}`);
			const data = await res.json();

			if (data.success && data.tasks.length > 0) {
				data.tasks.forEach((task) => {
					const card = document.createElement("div");
					card.className = `card-item glass hover-lift ${task.status === "done" ? "done" : ""}`;

					const left = document.createElement("div");
					left.className = "card-content-left";

					const checkbox = document.createElement("input");
					checkbox.type = "checkbox";
					checkbox.className = "checkbox-custom";
					checkbox.checked = task.status === "done";
					checkbox.addEventListener("change", () =>
						toggleTaskCompletion(task.id, checkbox.checked),
					);

					const text = document.createElement("div");
					text.className = "card-text";

					const title = document.createElement("div");
					title.className = "card-title";
					title.textContent = task.title;

					const desc = document.createElement("div");
					desc.className = "card-desc";
					desc.textContent = task.description || "説明なし";

					const meta = document.createElement("div");
					meta.className = "card-meta-row";

					if (task.due_date) {
						const due = document.createElement("span");
						due.className = "meta-item";

						const dueIcon = document.createElement("span");
						dueIcon.className = "material-symbols-outlined meta-icon";
						dueIcon.textContent = "calendar_today";

						const dueText = document.createTextNode(` 期限: ${task.due_date}`);

						due.appendChild(dueIcon);
						due.appendChild(dueText);
						meta.appendChild(due);
					}

					const pri = document.createElement("span");
					pri.className = "meta-item";

					const priIcon = document.createElement("span");
					priIcon.className = "material-symbols-outlined meta-icon";
					priIcon.textContent = "priority_high";

					// v2: priority は "high" | "medium" | "low" | null
					const priorityLabels = {
						high: "🔴 高",
						medium: "🟡 中",
						low: "🔵 低",
					};
					const priText = document.createTextNode(
						` 優先度: ${priorityLabels[task.priority] || "—"}`,
					);

					pri.appendChild(priIcon);
					pri.appendChild(priText);
					meta.appendChild(pri);

					// v2: tags はJSON文字列（パースしてチップ表示）
					let taskTags = [];
					try {
						taskTags = JSON.parse(task.tags || "[]");
						if (!Array.isArray(taskTags)) taskTags = [];
					} catch (err) {
						taskTags = [];
					}
					taskTags.forEach((tag) => {
						const chip = document.createElement("span");
						chip.className = "tag-chip";
						chip.textContent = `#${tag}`;
						meta.appendChild(chip);
					});

					text.appendChild(title);
					text.appendChild(desc);
					text.appendChild(meta);

					left.appendChild(checkbox);
					left.appendChild(text);

					const right = document.createElement("div");
					right.className = "card-actions-right";

					const btnTrash = document.createElement("button");
					btnTrash.className = "btn-trash";

					const trashIcon = document.createElement("span");
					trashIcon.className = "material-symbols-outlined";
					trashIcon.textContent = "delete";
					btnTrash.appendChild(trashIcon);

					btnTrash.addEventListener("click", () => handleDeleteTask(task.id));

					right.appendChild(btnTrash);

					card.appendChild(left);
					card.appendChild(right);
					tasksList.appendChild(card);
				});
			} else {
				const empty = document.createElement("div");
				empty.className = "glass";
				empty.textContent = "登録されているタスクがありません。";
				tasksList.appendChild(empty);
			}
		} catch (e) {
			console.error(e);
		}
	}

	// Filter Tasks
	document.querySelectorAll("[data-filter]").forEach((btn) => {
		btn.addEventListener("click", (e) => {
			document
				.querySelectorAll("[data-filter]")
				.forEach((b) => b.classList.remove("active"));
			btn.classList.add("active");
			const f = btn.getAttribute("data-filter");
			fetchTasksList(f);
		});
	});

	taskForm.addEventListener("submit", async (e) => {
		e.preventDefault();
		const title = document.getElementById("task-title").value.trim();
		const description = document
			.getElementById("task-description")
			.value.trim();
		const dueDate = document.getElementById("task-due").value;
		const priority =
			document.getElementById("task-priority").value || undefined;

		try {
			const res = await fetch("/api/tasks/add", {
				method: "POST",
				headers: { "Content-Type": "application/json" },
				body: JSON.stringify({ title, description, dueDate, priority }),
			});
			const data = await res.json();
			if (data.success) {
				closeModal(modalTask);
				taskForm.reset();
				fetchTasksList();
			}
		} catch (e) {
			console.error(e);
		}
	});

	async function toggleTaskCompletion(id, isChecked) {
		try {
			await fetch("/api/tasks/complete", {
				method: "POST",
				headers: { "Content-Type": "application/json" },
				body: JSON.stringify({ id }),
			});
			const activeFilter = document
				.querySelector("[data-filter].active")
				.getAttribute("data-filter");
			fetchTasksList(activeFilter);
		} catch (e) {
			console.error(e);
		}
	}

	async function handleDeleteTask(id) {
		if (!confirm("本当にこのタスクを削除しますか？")) return;
		try {
			await fetch("/api/tasks/delete", {
				method: "POST",
				headers: { "Content-Type": "application/json" },
				body: JSON.stringify({ id }),
			});
			const activeFilter = document
				.querySelector("[data-filter].active")
				.getAttribute("data-filter");
			fetchTasksList(activeFilter);
		} catch (e) {
			console.error(e);
		}
	}

	// F. SCHEDULES VIEW LOGIC (Fetch & CRUD)
	async function fetchSchedulesList(days = 7) {
		schedulesList.replaceChildren();

		try {
			const res = await fetch(`/api/schedules?days=${days}`);
			const data = await res.json();

			if (data.success && data.schedules.length > 0) {
				data.schedules.forEach((sched) => {
					const card = document.createElement("div");
					card.className = "card-item glass hover-lift";

					const left = document.createElement("div");
					left.className = "card-content-left";

					const icon = document.createElement("span");
					icon.className = "material-symbols-outlined list-card-icon";
					icon.style.fontSize = "1.8rem";
					icon.textContent = "event";

					const text = document.createElement("div");
					text.className = "card-text";

					const title = document.createElement("div");
					title.className = "card-title";
					title.textContent = sched.title;

					const desc = document.createElement("div");
					desc.className = "card-desc";
					desc.textContent = sched.description || "説明なし";

					const meta = document.createElement("div");
					meta.className = "card-meta-row";

					const time = document.createElement("span");
					time.className = "meta-item";

					const timeIcon = document.createElement("span");
					timeIcon.className = "material-symbols-outlined meta-icon";
					timeIcon.textContent = "schedule";

					const startClean = sched.start_at.slice(0, 16);
					const endClean = sched.end_at
						? ` 〜 ${sched.end_at.slice(11, 16)}`
						: "";
					const timeText = document.createTextNode(` ${startClean}${endClean}`);

					time.appendChild(timeIcon);
					time.appendChild(timeText);
					meta.appendChild(time);

					if (sched.google_calendar_id) {
						const cal = document.createElement("span");
						cal.className = "meta-item";

						const calIcon = document.createElement("span");
						calIcon.className = "material-symbols-outlined meta-icon";
						calIcon.textContent = "sync";

						const calText = document.createTextNode(" Google同期済み");

						cal.appendChild(calIcon);
						cal.appendChild(calText);
						meta.appendChild(cal);
					}

					text.appendChild(title);
					text.appendChild(desc);
					text.appendChild(meta);

					left.appendChild(icon);
					left.appendChild(text);

					const right = document.createElement("div");
					right.className = "card-actions-right";

					const btnTrash = document.createElement("button");
					btnTrash.className = "btn-trash";

					const trashIcon = document.createElement("span");
					trashIcon.className = "material-symbols-outlined";
					trashIcon.textContent = "delete";
					btnTrash.appendChild(trashIcon);

					btnTrash.addEventListener("click", () =>
						handleDeleteSchedule(sched.id),
					);

					right.appendChild(btnTrash);

					card.appendChild(left);
					card.appendChild(right);
					schedulesList.appendChild(card);
				});
			} else {
				const empty = document.createElement("div");
				empty.className = "glass";
				empty.textContent = "予定が登録されていません。";
				schedulesList.appendChild(empty);
			}
		} catch (e) {
			console.error(e);
		}
	}

	// Filter Schedules by Period
	document.querySelectorAll("[data-days]").forEach((btn) => {
		btn.addEventListener("click", () => {
			document
				.querySelectorAll("[data-days]")
				.forEach((b) => b.classList.remove("active"));
			btn.classList.add("active");
			const d = parseInt(btn.getAttribute("data-days"), 10);
			fetchSchedulesList(d);
		});
	});

	scheduleForm.addEventListener("submit", async (e) => {
		e.preventDefault();
		const title = document.getElementById("sched-title").value.trim();
		const description = document
			.getElementById("sched-description")
			.value.trim();
		const startAt =
			document.getElementById("sched-start").value.replace("T", " ") + ":00";
		const endInput = document.getElementById("sched-end").value;
		const endAt = endInput ? endInput.replace("T", " ") + ":00" : undefined;
		const remindBeforeMinutes = parseInt(
			document.getElementById("sched-remind").value,
			10,
		);

		try {
			const res = await fetch("/api/schedules/add", {
				method: "POST",
				headers: { "Content-Type": "application/json" },
				body: JSON.stringify({
					title,
					description,
					startAt,
					endAt,
					remindBeforeMinutes,
				}),
			});
			const data = await res.json();
			if (data.success) {
				closeModal(modalSchedule);
				scheduleForm.reset();
				fetchSchedulesList();
			}
		} catch (e) {
			console.error(e);
		}
	});

	async function handleDeleteSchedule(id) {
		if (
			!confirm(
				"本当にこの予定を削除しますか？ Googleカレンダー側からも削除されます。",
			)
		)
			return;
		try {
			await fetch("/api/schedules/delete", {
				method: "POST",
				headers: { "Content-Type": "application/json" },
				body: JSON.stringify({ id }),
			});
			const activeDays = parseInt(
				document.querySelector("[data-days].active").getAttribute("data-days"),
				10,
			);
			fetchSchedulesList(activeDays);
		} catch (e) {
			console.error(e);
		}
	}

	// G. EXPENSES VIEW LOGIC (Fetch & Receipt AI Scanning & Manual Add)
	async function fetchExpensesList() {
		expensesTableBody.replaceChildren();

		try {
			const [expRes, limitsRes, plansRes] = await Promise.all([
				fetch("/api/expenses"),
				fetch("/api/expenses/budget-limits"),
				fetch("/api/expenses/plans"),
			]);
			const data = await expRes.json();
			const limitsData = await limitsRes.json();
			const plansData = await plansRes.json();

			if (data.success) {
				expenseMonthTotal.textContent = `¥${data.total.toLocaleString()}`;

				// v2: 当月収入合計
				const incomeTotalEl = document.getElementById("expense-income-total");
				if (incomeTotalEl) {
					incomeTotalEl.textContent = `当月収入: ¥${(data.incomeTotal || 0).toLocaleString()}`;
				}

				// 支払い予定件数バッジ（v2: status=pending のみ取得・due_date）
				const plansDueCount = document.getElementById(
					"expense-plans-due-count",
				);
				if (plansDueCount && plansData.success) {
					const today = new Date().toISOString().slice(0, 10);
					const pendingPlans = (plansData.plans || []).filter(
						(p) => p.status === "pending",
					);
					const dueSoon = pendingPlans.filter((p) => p.due_date <= today);
					plansDueCount.textContent = `支払い予定: ${pendingPlans.length}件${dueSoon.length > 0 ? ` (本日以前 ${dueSoon.length}件)` : ""}`;
					plansDueCount.style.color =
						dueSoon.length > 0 ? "var(--color-red)" : "";
				}

				// カテゴリ別予算進捗バー
				renderCategoryBudgetBars(
					data.breakdown || [],
					limitsData.success ? limitsData.limits : [],
				);

				// Render Ledger table
				if (data.expenses && data.expenses.length > 0) {
					data.expenses.forEach((exp) => {
						const tr = document.createElement("tr");

						const tdDate = document.createElement("td");
						tdDate.textContent = exp.time
							? `${exp.date} ${exp.time.substring(0, 5)}`
							: exp.date;

						const tdCat = document.createElement("td");
						tdCat.textContent = exp.category;

						const tdDesc = document.createElement("td");
						tdDesc.textContent = exp.memo || "説明なし";

						const tdSource = document.createElement("td");
						tdSource.style.fontSize = "0.75rem";
						tdSource.style.fontFamily = "var(--font-family-mono)";

						const sourceIcon = document.createElement("span");
						sourceIcon.className = "material-symbols-outlined source-icon";
						const srcKey =
							exp.source === "receipt" || exp.source === "receipt_ocr"
								? "receipt"
								: exp.source === "plan"
									? "plan"
									: "manual";
						sourceIcon.textContent =
							srcKey === "receipt"
								? "photo_camera"
								: srcKey === "plan"
									? "event_available"
									: "web";

						tdSource.appendChild(sourceIcon);
						tdSource.appendChild(
							document.createTextNode(
								srcKey === "receipt"
									? "レシートAI"
									: srcKey === "plan"
										? "支払い予定"
										: "手動",
							),
						);

						const tdAmount = document.createElement("td");
						tdAmount.className = "amount-cell";
						if (exp.type === "income") {
							tdAmount.textContent = `+¥${exp.amount.toLocaleString()}`;
							tdAmount.style.color = "var(--color-green)";
						} else {
							tdAmount.textContent = `¥${exp.amount.toLocaleString()}`;
						}

						tr.appendChild(tdDate);
						tr.appendChild(tdCat);
						tr.appendChild(tdDesc);
						tr.appendChild(tdSource);
						tr.appendChild(tdAmount);
						expensesTableBody.appendChild(tr);
					});
				} else {
					const tr = document.createElement("tr");
					const td = document.createElement("td");
					td.colSpan = 5;
					td.style.textAlign = "center";
					td.textContent = "家計簿データが空です。";
					tr.appendChild(td);
					expensesTableBody.appendChild(tr);
				}
			}

			// 支払い予定テーブル
			if (plansData.success) {
				renderExpensePlans(plansData.plans || []);
			}
		} catch (e) {
			console.error(e);
		}
	}

	function renderCategoryBudgetBars(breakdown, limits) {
		const container = document.getElementById("category-budget-bars");
		if (!container) return;
		container.innerHTML = "";

		if (limits.length === 0) {
			container.innerHTML = `<p style="font-size:0.8rem;color:var(--text-secondary);">予算上限が設定されていません。「上限設定」ボタンから設定してください。</p>`;
			return;
		}

		const spendMap = {};
		breakdown.forEach((b) => {
			spendMap[b.category] = b.total;
		});

		limits.forEach((lim) => {
			const spent = spendMap[lim.category] || 0;
			const pct = Math.min((spent / lim.limit_amount) * 100, 100);
			const color =
				pct > 90
					? "var(--color-red)"
					: pct > 60
						? "#fbbf24"
						: "var(--color-primary)";

			const row = document.createElement("div");
			row.style.cssText = "display:flex;flex-direction:column;gap:3px;";
			row.innerHTML = `
        <div style="display:flex;justify-content:space-between;font-size:0.78rem;color:var(--text-secondary);">
          <span>${escapeHtml(lim.category)}</span>
          <span>¥${spent.toLocaleString()} / ¥${lim.limit_amount.toLocaleString()} (${Math.round(pct)}%)</span>
        </div>
        <div style="background:var(--surface-4dp);border-radius:3px;height:6px;overflow:hidden;">
          <div style="width:${pct}%;height:100%;background:${color};border-radius:3px;transition:width 0.4s ease;"></div>
        </div>
      `;
			container.appendChild(row);
		});
	}

	function renderExpensePlans(plans) {
		const tbody = document.getElementById("expense-plans-tbody");
		if (!tbody) return;
		tbody.innerHTML = "";
		const today = new Date().toISOString().slice(0, 10);

		if (plans.length === 0) {
			const tr = document.createElement("tr");
			const td = document.createElement("td");
			td.colSpan = 6;
			td.style.textAlign = "center";
			td.textContent = "支払い予定はありません。";
			tr.appendChild(td);
			tbody.appendChild(tr);
			return;
		}

		plans.forEach((plan) => {
			const tr = document.createElement("tr");
			const isPending = plan.status === "pending";
			const isOverdue = isPending && plan.due_date <= today;
			if (isOverdue) tr.style.background = "rgba(207,102,121,0.07)";

			const tdDate = document.createElement("td");
			tdDate.textContent = plan.due_date;
			if (isOverdue) tdDate.style.color = "var(--color-red)";
			if (plan.repeat_rule) {
				const repeatBadge = document.createElement("span");
				repeatBadge.className = "tag-chip";
				repeatBadge.style.marginLeft = "6px";
				repeatBadge.title = `繰り返し: ${plan.repeat_rule}`;
				repeatBadge.textContent = "🔁";
				tdDate.appendChild(repeatBadge);
			}

			const tdTitle = document.createElement("td");
			tdTitle.textContent = plan.title;

			const tdCat = document.createElement("td");
			tdCat.textContent = plan.category;

			const tdDesc = document.createElement("td");
			tdDesc.textContent = plan.memo || "—";
			tdDesc.style.color = "var(--text-secondary)";

			const tdAmount = document.createElement("td");
			tdAmount.className = "amount-cell";
			tdAmount.textContent = `¥${plan.amount.toLocaleString()}`;

			const tdActions = document.createElement("td");
			tdActions.style.cssText =
				"display:flex;justify-content:flex-end;align-items:center;gap:4px;";

			if (isPending) {
				const payBtn = document.createElement("button");
				payBtn.className = "btn btn-primary btn-sm";
				payBtn.style.cssText =
					"font-size:0.72rem;padding:3px 8px;line-height:1;";
				payBtn.textContent = "支払う";
				payBtn.addEventListener("click", async () => {
					if (
						!confirm(
							`「${plan.title}」¥${plan.amount.toLocaleString()} の支払いを完了しますか？\n家計簿に自動記録（消込）されます。`,
						)
					)
						return;
					try {
						const res = await fetch("/api/expenses/plans/pay", {
							method: "POST",
							headers: { "Content-Type": "application/json" },
							body: JSON.stringify({ id: plan.id }),
						});
						const result = await res.json();
						if (result.success) {
							fetchExpensesList();
						} else {
							alert("エラー: " + result.message);
						}
					} catch (e) {
						alert("通信エラーが発生しました。");
					}
				});
				tdActions.appendChild(payBtn);

				const delBtn = document.createElement("button");
				delBtn.className = "btn btn-secondary btn-sm";
				delBtn.style.cssText =
					"font-size:0.72rem;padding:3px 8px;line-height:1;display:inline-flex;align-items:center;";
				delBtn.title = "支払い予定をキャンセル";
				delBtn.innerHTML = `<span class="material-symbols-outlined" style="font-size:15px;line-height:1;">delete</span>`;
				delBtn.addEventListener("click", async () => {
					if (!confirm(`「${plan.title}」をキャンセルしますか？`)) return;
					try {
						const res = await fetch("/api/expenses/plans/delete", {
							method: "POST",
							headers: { "Content-Type": "application/json" },
							body: JSON.stringify({ id: plan.id }),
						});
						const result = await res.json();
						if (result.success) {
							fetchExpensesList();
						}
					} catch (e) {
						alert("通信エラーが発生しました。");
					}
				});
				tdActions.appendChild(delBtn);
			} else {
				const statusSpan = document.createElement("span");
				statusSpan.className =
					"status-badge " +
					(plan.status === "settled" ? "status-sent" : "status-cancelled");
				statusSpan.textContent =
					plan.status === "settled" ? "消込済み" : "キャンセル";
				tdActions.appendChild(statusSpan);
			}

			tr.appendChild(tdDate);
			tr.appendChild(tdTitle);
			tr.appendChild(tdCat);
			tr.appendChild(tdDesc);
			tr.appendChild(tdAmount);
			tr.appendChild(tdActions);
			tbody.appendChild(tr);
		});
	}

	expenseForm.addEventListener("submit", async (e) => {
		e.preventDefault();
		const amount = parseInt(document.getElementById("exp-amount").value, 10);
		const category = document.getElementById("exp-category").value;
		const description = document.getElementById("exp-description").value.trim();
		const date = document.getElementById("exp-date").value;
		const type =
			document.getElementById("exp-type").value === "income"
				? "income"
				: "expense";

		const now = new Date();
		const pad = (n) => n.toString().padStart(2, "0");
		const time = `${pad(now.getHours())}:${pad(now.getMinutes())}:${pad(now.getSeconds())}`;

		try {
			const res = await fetch("/api/expenses/add", {
				method: "POST",
				headers: { "Content-Type": "application/json" },
				body: JSON.stringify({
					amount,
					category,
					description,
					date,
					time,
					type,
				}),
			});
			const data = await res.json();
			if (data.success) {
				expenseForm.reset();
				expDateInput.value = new Date().toISOString().slice(0, 10);
				fetchExpensesList();
			}
		} catch (e) {
			console.error(e);
		}
	});

	// ── 支払い予定モーダル ──
	const modalExpensePlan = document.getElementById("modal-expense-plan");
	const expensePlanForm = document.getElementById("expense-plan-form");
	const btnNewExpensePlan = document.getElementById("btn-new-expense-plan");
	const btnCloseExpensePlan = document.getElementById("btn-close-expense-plan");

	if (btnNewExpensePlan && modalExpensePlan) {
		btnNewExpensePlan.addEventListener("click", () => {
			document.getElementById("plan-date").value = new Date()
				.toISOString()
				.slice(0, 10);
			modalExpensePlan.classList.add("active");
		});
	}
	if (btnCloseExpensePlan && modalExpensePlan) {
		btnCloseExpensePlan.addEventListener("click", () =>
			modalExpensePlan.classList.remove("active"),
		);
	}
	if (expensePlanForm) {
		expensePlanForm.addEventListener("submit", async (e) => {
			e.preventDefault();
			const title = document.getElementById("plan-title").value.trim();
			const amount = parseInt(document.getElementById("plan-amount").value, 10);
			const category = document.getElementById("plan-category").value;
			const plannedDate = document.getElementById("plan-date").value;
			const description = document
				.getElementById("plan-description")
				.value.trim();
			try {
				const res = await fetch("/api/expenses/plans/add", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify({
						title,
						amount,
						category,
						plannedDate,
						description,
					}),
				});
				const data = await res.json();
				if (data.success) {
					expensePlanForm.reset();
					modalExpensePlan.classList.remove("active");
					fetchExpensesList();
				} else {
					alert("エラー: " + data.message);
				}
			} catch (e) {
				console.error(e);
			}
		});
	}

	// ── 予算上限設定モーダル ──
	const modalBudgetSettings = document.getElementById("modal-budget-settings");
	const budgetLimitForm = document.getElementById("budget-limit-form");
	const btnOpenBudgetSettings = document.getElementById(
		"btn-open-budget-settings",
	);
	const btnCloseBudgetSettings = document.getElementById(
		"btn-close-budget-settings",
	);

	if (btnOpenBudgetSettings && modalBudgetSettings) {
		btnOpenBudgetSettings.addEventListener("click", async () => {
			await renderBudgetSettingsList();
			modalBudgetSettings.classList.add("active");
		});
	}
	if (btnCloseBudgetSettings && modalBudgetSettings) {
		btnCloseBudgetSettings.addEventListener("click", () =>
			modalBudgetSettings.classList.remove("active"),
		);
	}

	async function renderBudgetSettingsList() {
		const list = document.getElementById("budget-settings-list");
		if (!list) return;
		try {
			const res = await fetch("/api/expenses/budget-limits");
			const data = await res.json();
			list.innerHTML = "";
			if (!data.success || data.limits.length === 0) {
				list.innerHTML = `<p style="font-size:0.8rem;color:var(--text-secondary);padding: 8px 0;">設定済みの上限はありません。</p>`;
				return;
			}
			data.limits.forEach((lim) => {
				const row = document.createElement("div");
				row.style.cssText =
					"display:flex;align-items:center;justify-content:space-between;padding:10px 0;border-bottom:1px solid var(--border-divider);";
				row.innerHTML = `
          <span style="font-size:0.9rem;font-weight:500;color:var(--text-primary);">${escapeHtml(lim.category)}</span>
          <div class="limit-right-content" style="display:flex;align-items:center;gap:12px;">
            <span style="font-size:0.9rem;font-weight:700;color:var(--text-primary);">¥${lim.limit_amount.toLocaleString()}</span>
          </div>
        `;
				const delBtn = document.createElement("button");
				delBtn.className = "btn btn-secondary btn-sm";
				delBtn.style.cssText =
					"font-size:0.72rem;padding:4px 10px;text-transform:none;letter-spacing:normal;font-weight:normal;height:auto;";
				delBtn.textContent = "削除";
				delBtn.addEventListener("click", async () => {
					try {
						await fetch("/api/expenses/budget-limits/delete", {
							method: "POST",
							headers: { "Content-Type": "application/json" },
							body: JSON.stringify({ category: lim.category }),
						});
						await renderBudgetSettingsList();
						fetchExpensesList();
					} catch (e) {
						console.error(e);
					}
				});
				row.querySelector(".limit-right-content").appendChild(delBtn);
				list.appendChild(row);
			});
		} catch (e) {
			console.error(e);
		}
	}

	if (budgetLimitForm) {
		budgetLimitForm.addEventListener("submit", async (e) => {
			e.preventDefault();
			const category = document.getElementById("budget-category").value;
			const limitAmount = parseInt(
				document.getElementById("budget-limit-amount").value,
				10,
			);
			if (!category || isNaN(limitAmount) || limitAmount <= 0) {
				alert("カテゴリーと有効な上限金額を入力してください。");
				return;
			}
			try {
				const res = await fetch("/api/expenses/budget-limits", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify({ category, limitAmount }),
				});
				const data = await res.json();
				if (data.success) {
					document.getElementById("budget-limit-amount").value = "";
					await renderBudgetSettingsList();
					fetchExpensesList();
				} else {
					alert("エラー: " + data.message);
				}
			} catch (e) {
				console.error(e);
			}
		});
	}

	// AI Receipt Dropzone drag-drop
	if (receiptDropzone) {
		receiptDropzone.addEventListener("click", () => receiptFileInput.click());

		receiptDropzone.addEventListener("dragover", (e) => {
			e.preventDefault();
			receiptDropzone.classList.add("dragover");
		});

		receiptDropzone.addEventListener("dragleave", () => {
			receiptDropzone.classList.remove("dragover");
		});

		receiptDropzone.addEventListener("drop", (e) => {
			e.preventDefault();
			receiptDropzone.classList.remove("dragover");
			if (e.dataTransfer.files.length > 0) {
				handleReceiptScan(e.dataTransfer.files[0]);
			}
		});

		receiptFileInput.addEventListener("change", (e) => {
			if (e.target.files.length > 0) {
				handleReceiptScan(e.target.files[0]);
			}
		});
	}

	async function handleReceiptScan(file) {
		if (!file.type.startsWith("image/")) {
			alert("レシート解析は画像ファイル（PNG/JPEG）のみ対応しています。");
			return;
		}

		scanStatus.classList.remove("hidden");
		scanStatusText.textContent =
			"レシート画像をアップロードしてAIに渡しています...";

		const reader = new FileReader();
		reader.onload = async () => {
			const base64Str = reader.result.split(",")[1];
			const mimeType = file.type;

			try {
				const res = await fetch("/api/expenses/upload-receipt", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify({
						imageBase64: base64Str,
						mimeType: mimeType,
						additionalText: "WEB管理画面からアップロードされた画像レシート",
					}),
				});
				const data = await res.json();

				if (data.success) {
					scanStatus.classList.add("hidden");
					receiptAiResponse.textContent = data.response;
					openModal(modalReceiptResult);
					fetchExpensesList();
				} else {
					throw new Error(data.message);
				}
			} catch (err) {
				scanStatus.classList.add("hidden");
				alert(`解析エラーが発生しました:\n${err.message}`);
			}
		};
		reader.readAsDataURL(file);
	}

	// アカウント管理ページ（表示名）の値を投入する。Bot文脈に依存しないため独立に取得。
	async function fetchAccountSettings() {
		try {
			const res = await fetch("/api/status");
			const data = await res.json();
			if (data.success && data.user) {
				const el = document.getElementById("config-profile-username");
				if (el) el.value = data.user.username;
			}
		} catch (err) {
			console.error("アカウント設定の取得に失敗しました:", err);
		}
	}

	// H. SYSTEM CONFIG POLL
	async function fetchConfigSettings() {
		try {
			const res = await fetch("/api/status");
			const data = await res.json();

			if (data.success) {
				// Fill individual config form values
				document.getElementById("gemini-model-select").value =
					data.config.geminiModel;

				document.getElementById("backup-enable").checked =
					data.config.backupEnabled;
				document.getElementById("backup-folder-id").value =
					data.config.backupFolderId === "未設定"
						? ""
						: data.config.backupFolderId;
				document.getElementById("backup-interval-hours").value =
					data.config.backupIntervalHours ?? 24;
				document.getElementById("backup-generations").value =
					data.config.backupGenerations ?? 7;
				const backupLastRun = document.getElementById("backup-last-run");
				if (backupLastRun) {
					backupLastRun.textContent = `最終実行: ${data.config.backupLastRunAt || "—"}`;
				}

				// アシスタント設定（/api/settings/user の現在値）
				const richReplyEl = document.getElementById("user-rich-reply");
				if (richReplyEl)
					richReplyEl.checked = data.config.richReplyEnabled !== false;
				const remindDefaultEl = document.getElementById("user-remind-default");
				if (remindDefaultEl)
					remindDefaultEl.value = data.config.remindDefaultMinutes ?? 10;
				const notifyTypeEl = document.getElementById("user-notify-type");
				const notifyIdEl = document.getElementById("user-notify-id");
				if (notifyTypeEl)
					notifyTypeEl.value =
						(data.config.notifyTarget && data.config.notifyTarget.type) || "dm";
				if (notifyIdEl)
					notifyIdEl.value =
						(data.config.notifyTarget && data.config.notifyTarget.id) || "";

				// UI access control for system_default bot settings
				const isSystemDefault = window.currentBotId === "system_default";
				const isAdmin = activeUserRole === "admin";
				const configDiscordCard = document.getElementById(
					"config-discord-card",
				);

				if (configDiscordCard) {
					if (isSystemDefault && !isAdmin) {
						configDiscordCard.classList.add("hidden");
					} else {
						configDiscordCard.classList.remove("hidden");
					}
				}

				// system_default を非管理者が見ている場合、バックアップを隠す
				// （Google連携カードはBot個別画面から廃止し「Bot統合管理」へ集約）
				const restrictedForNonAdmin = isSystemDefault && !isAdmin;
				["config-backup-section"].forEach((id) => {
					const el = document.getElementById(id);
					if (el) el.classList.toggle("hidden", restrictedForNonAdmin);
				});

				// Fetch and fill Discord token config (v2: トークンのみ)
				if (!isSystemDefault || isAdmin) {
					try {
						fetch("/api/settings/discord")
							.then((res) => res.json())
							.then((discordData) => {
								if (discordData.success) {
									document.getElementById("discord-token").value =
										discordData.tokenMasked;
								}
							})
							.catch((err) =>
								console.error("Failed to load Discord settings:", err),
							);
					} catch (err) {
						console.error("Failed to load Discord settings:", err);
					}
				}
			}

			// Load AI Credentials List & Bot共有設定
			fetchCredentialsSettings();
			fetchBotShares();
			// Bot属性（プリセット）と汎用モード設定
			fetchBotAttributeConfig();
		} catch (e) {
			console.error(e);
		}
	}

	// ==========================================
	// Bot属性（プリセット）+ 汎用モード（MCPアシスタント）設定
	// ==========================================

	// Bot導入リンクで付与する権限（このBotが実際に使うAPIに対応:
	// チャンネル表示(1024)+メッセージ送信(2048)+埋め込みリンク(16384)+ファイル添付(32768)+メッセージ履歴閲覧(65536) = 117760）
	const BOT_INVITE_PERMISSIONS = "117760";

	/** Bot招待リンク・プロフィールURLカードを更新する。application_id 未取得なら同期を促す */
	function updateBotInviteCard(bot) {
		const card = document.getElementById("config-bot-invite-card");
		if (!card) return;
		const fields = document.getElementById("bot-invite-fields");
		const empty = document.getElementById("bot-invite-empty");
		card.classList.remove("hidden");

		const appId = bot.discord_application_id;
		if (appId) {
			const inviteUrl = `https://discord.com/oauth2/authorize?client_id=${encodeURIComponent(appId)}&scope=bot&permissions=${BOT_INVITE_PERMISSIONS}`;
			const profileUrl = `https://discord.com/users/${encodeURIComponent(appId)}`;
			const inviteInput = document.getElementById("bot-invite-url");
			const profileInput = document.getElementById("bot-profile-url");
			const inviteOpen = document.getElementById("bot-invite-open");
			const profileOpen = document.getElementById("bot-profile-open");
			if (inviteInput) inviteInput.value = inviteUrl;
			if (profileInput) profileInput.value = profileUrl;
			if (inviteOpen) inviteOpen.href = inviteUrl;
			if (profileOpen) profileOpen.href = profileUrl;
			if (fields) fields.classList.remove("hidden");
			if (empty) empty.classList.add("hidden");
		} else {
			if (fields) fields.classList.add("hidden");
			if (empty) empty.classList.remove("hidden");
		}
	}

	/** 現在のBotの属性カード・汎用モード設定カードの表示を更新する（owner のみ表示） */
	async function fetchBotAttributeConfig() {
		const attrCard = document.getElementById("config-bot-attribute-card");
		const assistantCard = document.getElementById("config-assistant-card");
		const nameCard = document.getElementById("config-bot-name-card");
		const inviteCard = document.getElementById("config-bot-invite-card");
		// 移設した汎用モード関連カード（Discord連携タブ・ペルソナタブに配置）
		const assistantDiscordCard = document.getElementById(
			"config-assistant-discord-card",
		);
		const assistantPersonaCard = document.getElementById(
			"config-assistant-persona-card",
		);
		if (attrCard) attrCard.classList.add("hidden");
		if (assistantCard) assistantCard.classList.add("hidden");
		if (nameCard) nameCard.classList.add("hidden");
		if (inviteCard) inviteCard.classList.add("hidden");
		if (assistantDiscordCard) assistantDiscordCard.classList.add("hidden");
		if (assistantPersonaCard) assistantPersonaCard.classList.add("hidden");

		const botId = window.currentBotId;
		if (
			!botId ||
			botId === "system_default" ||
			botId.startsWith("bot_default_")
		)
			return;

		try {
			const res = await originalFetch("/api/bots");
			const data = await res.json();
			if (!data.success) return;
			const bot = (data.bots || []).find((b) => b.id === botId);
			if (!bot) return;

			// ── Bot 招待リンク・プロフィールURLカード（アクセス権のあるBotに表示） ──
			updateBotInviteCard(bot);

			// プリセットが変わっている可能性に備えて localStorage を同期
			if (bot.preset && bot.preset !== currentBotPreset()) {
				localStorage.setItem("currentBotPreset", bot.preset);
				updateSidebarBotBranding();
			}

			const isOwner = bot.user_id === activeUserId;
			const isAdminUser = activeUserRole === "admin";
			if (!isOwner && !isAdminUser) return; // 属性の変更は owner / Admin のみ（§4.1）

			// ── Bot登録名カード ── 現在の登録名を反映。表示名復元用に discord_username も保持。
			if (nameCard) {
				nameCard.classList.remove("hidden");
				const nameInput = document.getElementById("bot-name-input");
				if (nameInput) nameInput.value = bot.name || "";
				const nameForm = document.getElementById("bot-name-form");
				if (nameForm)
					nameForm.dataset.discordUsername = bot.discord_username || "";
				const fb = document.getElementById("bot-name-feedback");
				if (fb) {
					fb.style.display = "none";
					fb.textContent = "";
				}
			}

			// ── Bot属性カード ──
			if (attrCard) {
				attrCard.classList.remove("hidden");
				const badge = document.getElementById("bot-attribute-current-badge");
				if (badge) badge.textContent = bot.preset_display_name || "";
				const select = document.getElementById("bot-attribute-preset-select");
				if (select) {
					try {
						const pres = await originalFetch("/api/bots/presets").then((r) =>
							r.json(),
						);
						if (pres.success) {
							select.innerHTML = "";
							pres.presets.forEach((p) => {
								const opt = document.createElement("option");
								opt.value = p.id;
								opt.textContent = `${p.displayName}（${p.capabilities.join(" + ")}）`;
								select.appendChild(opt);
							});
						}
					} catch (e) {}
					select.value = bot.preset || "secretary";
				}
			}

			// ── 汎用モード設定カード（本体は「Bot設定」、ギルド/メンバー/ノートは「Discord連携」、
			//     Bot単位ペルソナは「ペルソナ」タブに分割配置。loadAssistantConfig が ID 単位で全カードを描画する） ──
			if (bot.preset === "mcp_assistant") {
				if (assistantCard) assistantCard.classList.remove("hidden");
				if (assistantDiscordCard)
					assistantDiscordCard.classList.remove("hidden");
				if (assistantPersonaCard)
					assistantPersonaCard.classList.remove("hidden");
				await loadAssistantConfig(botId);
			}
		} catch (e) {
			console.error("Bot属性設定の読み込みに失敗しました:", e);
		}
	}

	/**
	 * 「Discord連携」タブの読み込み。Discordトークン（マスク表示）を反映し、
	 * 招待リンクカード＋汎用モードのギルド/メンバー/共有ノートカードを fetchBotAttributeConfig 経由で描画する。
	 */
	async function fetchDiscordSettings() {
		const botId = window.currentBotId;
		const isSystemDefault = !botId || botId === "system_default";
		const isAdminUser = activeUserRole === "admin";

		// Discord 独自Botトークン（マスク表示）。system_default は管理者のみ取得可。
		const tokenInput = document.getElementById("discord-token");
		if (tokenInput && (!isSystemDefault || isAdminUser)) {
			try {
				const res = await fetch("/api/settings/discord");
				const data = await res.json();
				if (data.success) tokenInput.value = data.tokenMasked;
			} catch (err) {
				console.error("Failed to load Discord settings:", err);
			}
		}

		// 招待リンク・プロフィールカード＋汎用モードDiscord設定（ギルド/メンバー/共有ノート）を描画
		await fetchBotAttributeConfig();
	}

	/** 汎用モード設定タブの内容を取得して描画する */
	async function loadAssistantConfig(botId) {
		try {
			const res = await originalFetch(
				`/api/bots/assistant-config?botId=${encodeURIComponent(botId)}`,
			);
			const data = await res.json();
			if (!data.success) return;

			// 警告表示（応答に必要な設定が欠けている場合 §4.3.3）
			const warnings = document.getElementById("assistant-warnings");
			if (warnings) {
				warnings.innerHTML = "";
				const warn = (text) => {
					const div = document.createElement("div");
					div.className = "field-sub";
					div.style.color = "#f59e0b";
					div.textContent = text;
					warnings.appendChild(div);
				};
				if (!data.has_gemini_key)
					warn(
						"⚠️ Bot専用のGemini APIキーが未設定です。設定されるまでこのBotは応答しません。",
					);
				if (!data.has_discord_token)
					warn(
						"⚠️ 独自のDiscord Botトークンが未設定です。汎用モードは専用のDiscordクライアントとして動作するため、「Discord連携」ページの「Discord 独自Bot設定」から設定してください。",
					);
				if ((data.guilds || []).length === 0)
					warn(
						"⚠️ 応答許可ギルドが未設定です。許可したギルドでのみ応答します。",
					);
			}

			// APIキー（マスク表示）
			const keyInput = document.getElementById("assistant-gemini-key");
			if (keyInput) keyInput.value = data.has_gemini_key ? "••••••••••••" : "";

			// ペルソナ選択（自身のペルソナ + 公開ペルソナ §4.4）
			const personaSelect = document.getElementById("assistant-persona-select");
			if (personaSelect) {
				personaSelect.innerHTML = "";
				const defaultOpt = document.createElement("option");
				defaultOpt.value = "";
				defaultOpt.textContent = "（デフォルトペルソナ）";
				personaSelect.appendChild(defaultOpt);
				(data.personas || []).forEach((p) => {
					const opt = document.createElement("option");
					opt.value = String(p.id);
					opt.textContent = p.name;
					personaSelect.appendChild(opt);
				});
				personaSelect.value = data.persona_id ? String(data.persona_id) : "";
			}

			// MCPサーバー一覧（読み取り専用表示。登録・編集はMCPサーバータブで行う）
			const mcpList = document.getElementById("assistant-mcp-list");
			if (mcpList) {
				mcpList.innerHTML = "";

				// タブへの案内文
				const guide = document.createElement("p");
				guide.className = "field-sub";
				guide.style.marginBottom = "8px";
				guide.textContent =
					"MCPサーバーの登録・編集・削除は「MCPサーバー」タブで、表示中のBotごとに行います。";
				mcpList.appendChild(guide);

				const servers = data.mcp_servers || [];
				if (servers.length === 0) {
					const empty = document.createElement("span");
					empty.className = "field-sub";
					empty.textContent =
						"このBot専用のMCPサーバーは未登録です。「MCPサーバー」タブ（このBotを選択した状態）から登録してください。";
					mcpList.appendChild(empty);
				}
				servers.forEach((s) => {
					const row = document.createElement("div");
					row.style.cssText =
						"display:flex; align-items:center; gap:8px; padding:4px 0;";
					const nameSpan = document.createElement("span");
					nameSpan.style.cssText = "font-size:0.9rem;";
					nameSpan.textContent = s.name;
					const labelSpan = document.createElement("span");
					labelSpan.className = "field-sub";
					labelSpan.style.cssText = "font-size:0.78rem;";
					const labelParts = [];
					if (s.system) labelParts.push("システムレベル・常時利用可");
					if (!s.enabled) labelParts.push("無効化中");
					if (labelParts.length > 0)
						labelSpan.textContent = `（${labelParts.join("・")}）`;
					row.appendChild(nameSpan);
					if (labelParts.length > 0) row.appendChild(labelSpan);
					mcpList.appendChild(row);
				});
			}

			renderAssistantGuilds(botId, data.guilds || []);
			renderAssistantMembers(botId, data.members || [], data.guilds || []);

			// 共有ノートのギルド選択
			const noteGuildSelect = document.getElementById(
				"assistant-note-guild-select",
			);
			if (noteGuildSelect) {
				noteGuildSelect.innerHTML = "";
				(data.guilds || []).forEach((g) => {
					const opt = document.createElement("option");
					opt.value = g.guild_id;
					opt.textContent = `ギルド ${g.guild_id}`;
					noteGuildSelect.appendChild(opt);
				});
				if ((data.guilds || []).length > 0) {
					loadAssistantGuildNote(botId, noteGuildSelect.value);
				} else {
					const noteArea = document.getElementById("assistant-guild-note");
					if (noteArea) noteArea.value = "";
				}
			}

			// 利用量（直近14日・日次）
			const usageEl = document.getElementById("assistant-usage");
			if (usageEl) {
				const usage = data.usage || [];
				if (usage.length === 0) {
					usageEl.innerHTML = `<span class="field-sub">まだ利用がありません。</span>`;
				} else {
					usageEl.innerHTML = usage
						.map(
							(u) =>
								`<div style="display:flex; justify-content:space-between; max-width:280px;"><span class="field-sub">${escapeHtml(u.date)}</span><span>${u.count} 回</span></div>`,
						)
						.join("");
				}
			}
			const rateEl = document.getElementById("assistant-rate-limits");
			if (rateEl && data.rate_limits) {
				rateEl.textContent = `レート制限: ユーザー ${data.rate_limits.userPerMinute}回/分・${data.rate_limits.userPerDay}回/日、ギルド ${data.rate_limits.guildPerDay}回/日（Admin設定で変更可）`;
			}
		} catch (e) {
			console.error("汎用モード設定の読み込みに失敗しました:", e);
		}
	}

	function renderAssistantGuilds(botId, guilds) {
		const list = document.getElementById("assistant-guild-list");
		if (!list) return;
		list.innerHTML = "";
		if (guilds.length === 0) {
			list.innerHTML = `<span class="field-sub">許可ギルドが未登録です。</span>`;
			return;
		}
		guilds.forEach((g) => {
			const row = document.createElement("div");
			row.style.cssText =
				"display:flex; align-items:center; justify-content:space-between; gap:10px;";
			const label = document.createElement("span");
			label.style.fontFamily = "var(--font-family-mono)";
			label.textContent = g.guild_id;
			const removeBtn = document.createElement("button");
			removeBtn.className = "btn btn-secondary btn-sm";
			removeBtn.textContent = "削除";
			removeBtn.addEventListener("click", async () => {
				if (!confirm(`ギルド ${g.guild_id} を応答許可リストから削除しますか？`))
					return;
				await postAssistantAction("/api/bots/assistant/guilds", {
					botId,
					guildId: g.guild_id,
					action: "remove",
				});
			});
			row.appendChild(label);
			row.appendChild(removeBtn);
			list.appendChild(row);
		});
	}

	function renderAssistantMembers(botId, members, guilds) {
		// メンバー追加用のギルド選択
		const guildSelect = document.getElementById(
			"assistant-member-guild-select",
		);
		if (guildSelect) {
			const prev = guildSelect.value;
			guildSelect.innerHTML = "";
			guilds.forEach((g) => {
				const opt = document.createElement("option");
				opt.value = g.guild_id;
				opt.textContent = `ギルド ${g.guild_id}`;
				guildSelect.appendChild(opt);
			});
			if (prev && guilds.some((g) => g.guild_id === prev))
				guildSelect.value = prev;
		}

		const list = document.getElementById("assistant-member-list");
		if (!list) return;
		list.innerHTML = "";
		if (members.length === 0) {
			list.innerHTML = `<span class="field-sub">登録済みの利用メンバーはいません（あなたは常に利用できます）。</span>`;
			return;
		}
		members.forEach((m) => {
			const row = document.createElement("div");
			row.style.cssText =
				"display:flex; align-items:center; justify-content:space-between; gap:10px;";
			const label = document.createElement("span");
			label.className = "field-sub";
			label.innerHTML = `<span style="font-family:var(--font-family-mono);">${escapeHtml(m.user_id)}</span> @ ギルド ${escapeHtml(m.guild_id)}`;
			const removeBtn = document.createElement("button");
			removeBtn.className = "btn btn-secondary btn-sm";
			removeBtn.textContent = "削除";
			removeBtn.addEventListener("click", async () => {
				if (!confirm(`ユーザー ${m.user_id} を利用メンバーから削除しますか？`))
					return;
				await postAssistantAction("/api/bots/assistant/members", {
					botId,
					guildId: m.guild_id,
					userId: m.user_id,
					action: "remove",
				});
			});
			row.appendChild(label);
			row.appendChild(removeBtn);
			list.appendChild(row);
		});
	}

	/** 汎用モード設定のPOST + 再読み込みの共通処理 */
	async function postAssistantAction(url, body) {
		try {
			const res = await originalFetch(url, {
				method: "POST",
				headers: { "Content-Type": "application/json" },
				body: JSON.stringify(body),
			});
			const data = await res.json();
			if (!data.success) {
				alert(data.message || "操作に失敗しました。");
			}
			await loadAssistantConfig(body.botId);
			return data;
		} catch (e) {
			alert("通信エラーが発生しました。");
			return { success: false };
		}
	}

	async function loadAssistantGuildNote(botId, guildId) {
		const noteArea = document.getElementById("assistant-guild-note");
		if (!noteArea || !guildId) return;
		try {
			const res = await originalFetch(
				`/api/bots/assistant/guild-note?botId=${encodeURIComponent(botId)}&guildId=${encodeURIComponent(guildId)}`,
			);
			const data = await res.json();
			noteArea.value = data.success ? data.content : "";
		} catch (e) {
			noteArea.value = "";
		}
	}

	// ── 汎用モード設定のイベントハンドラ（要素は静的なので一度だけ登録） ──

	// ── Bot招待リンク・プロフィールURLカードの操作 ──
	function copyInputValue(inputId, btn) {
		const input = document.getElementById(inputId);
		if (!input || !input.value) return;
		const flash = () => {
			const orig = btn.textContent;
			btn.textContent = "コピー済み";
			setTimeout(() => {
				btn.textContent = orig;
			}, 1500);
		};
		if (navigator.clipboard && navigator.clipboard.writeText) {
			navigator.clipboard
				.writeText(input.value)
				.then(flash)
				.catch(() => {
					input.select();
					try {
						document.execCommand("copy");
						flash();
					} catch (e) {}
				});
		} else {
			input.select();
			try {
				document.execCommand("copy");
				flash();
			} catch (e) {}
		}
	}

	const btnCopyInvite = document.getElementById("btn-copy-bot-invite");
	if (btnCopyInvite)
		btnCopyInvite.addEventListener("click", () =>
			copyInputValue("bot-invite-url", btnCopyInvite),
		);
	const btnCopyProfile = document.getElementById("btn-copy-bot-profile");
	if (btnCopyProfile)
		btnCopyProfile.addEventListener("click", () =>
			copyInputValue("bot-profile-url", btnCopyProfile),
		);

	const btnInviteSync = document.getElementById("btn-invite-sync");
	if (btnInviteSync) {
		btnInviteSync.addEventListener("click", async () => {
			const botId = window.currentBotId;
			if (!botId) return;
			btnInviteSync.disabled = true;
			const orig = btnInviteSync.textContent;
			btnInviteSync.textContent = "同期中...";
			try {
				const res = await originalFetch("/api/bots/sync-discord", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify({ botId }),
				});
				const data = await res.json();
				if (data.success) {
					fetchBotAttributeConfig();
					fetchBotList();
				} else {
					alert(
						data.message ||
							"同期に失敗しました。Botが起動しているか確認してください。",
					);
				}
			} catch (e) {
				alert("通信エラーが発生しました。");
			} finally {
				btnInviteSync.disabled = false;
				btnInviteSync.textContent = orig;
			}
		});
	}

	const btnSaveBotAttribute = document.getElementById("btn-save-bot-attribute");
	if (btnSaveBotAttribute) {
		btnSaveBotAttribute.addEventListener("click", async () => {
			const select = document.getElementById("bot-attribute-preset-select");
			if (!select || !select.value) return;
			const preset = select.value;
			const presetLabel =
				select.selectedIndex >= 0
					? select.options[select.selectedIndex].textContent
					: preset;
			if (
				!confirm(
					`Botの属性を「${presetLabel}」へ変更しますか？\n機能セットが切り替わります（会話の文脈は属性ごとに分離されます）。`,
				)
			)
				return;
			try {
				const res = await originalFetch("/api/bots/attributes", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify({ botId: window.currentBotId, preset }),
				});
				const data = await res.json();
				alert(
					data.message ||
						(data.success ? "変更しました。" : "変更に失敗しました。"),
				);
				if (data.success) {
					localStorage.setItem("currentBotPreset", preset);
					updateSidebarBotBranding();
					fetchBotAttributeConfig();
				}
			} catch (e) {
				alert("通信エラーが発生しました。");
			}
		});
	}

	const btnSaveAssistantKey = document.getElementById("btn-save-assistant-key");
	if (btnSaveAssistantKey) {
		btnSaveAssistantKey.addEventListener("click", async () => {
			const input = document.getElementById("assistant-gemini-key");
			const apiKey = input ? input.value : "";
			if (
				!apiKey.trim() &&
				!confirm(
					"APIキーを削除しますか？ 削除するとこのBotは応答を停止します。",
				)
			)
				return;
			await postAssistantAction("/api/bots/assistant/gemini-key", {
				botId: window.currentBotId,
				apiKey,
			});
		});
	}

	const btnSaveAssistantPersona = document.getElementById(
		"btn-save-assistant-persona",
	);
	if (btnSaveAssistantPersona) {
		btnSaveAssistantPersona.addEventListener("click", async () => {
			const select = document.getElementById("assistant-persona-select");
			const personaId = select && select.value ? Number(select.value) : null;
			await postAssistantAction("/api/bots/assistant/persona", {
				botId: window.currentBotId,
				personaId,
			});
		});
	}

	const btnAddAssistantGuild = document.getElementById(
		"btn-add-assistant-guild",
	);
	if (btnAddAssistantGuild) {
		btnAddAssistantGuild.addEventListener("click", async () => {
			const input = document.getElementById("assistant-guild-input");
			const guildId = input ? input.value.trim() : "";
			if (!/^\d{5,25}$/.test(guildId)) {
				alert(
					"ギルドID（数字）を入力してください。Discordの開発者モードでサーバーを右クリック →「IDをコピー」で取得できます。",
				);
				return;
			}
			const result = await postAssistantAction("/api/bots/assistant/guilds", {
				botId: window.currentBotId,
				guildId,
				action: "add",
			});
			if (result.success && input) input.value = "";
		});
	}

	const btnAddAssistantMember = document.getElementById(
		"btn-add-assistant-member",
	);
	if (btnAddAssistantMember) {
		btnAddAssistantMember.addEventListener("click", async () => {
			const guildSelect = document.getElementById(
				"assistant-member-guild-select",
			);
			const input = document.getElementById("assistant-member-input");
			const guildId = guildSelect ? guildSelect.value : "";
			const userId = input ? input.value.trim() : "";
			if (!guildId) {
				alert("先に応答許可ギルドを追加してください。");
				return;
			}
			if (!/^\d{5,25}$/.test(userId)) {
				alert("ユーザーID（数字）を入力してください。");
				return;
			}
			const result = await postAssistantAction("/api/bots/assistant/members", {
				botId: window.currentBotId,
				guildId,
				userId,
				action: "add",
			});
			if (result.success && input) input.value = "";
		});
	}

	const assistantNoteGuildSelect = document.getElementById(
		"assistant-note-guild-select",
	);
	if (assistantNoteGuildSelect) {
		assistantNoteGuildSelect.addEventListener("change", () => {
			loadAssistantGuildNote(
				window.currentBotId,
				assistantNoteGuildSelect.value,
			);
		});
	}

	const btnSaveAssistantNote = document.getElementById(
		"btn-save-assistant-note",
	);
	if (btnSaveAssistantNote) {
		btnSaveAssistantNote.addEventListener("click", async () => {
			const guildSelect = document.getElementById(
				"assistant-note-guild-select",
			);
			const noteArea = document.getElementById("assistant-guild-note");
			const guildId = guildSelect ? guildSelect.value : "";
			if (!guildId) {
				alert("先に応答許可ギルドを追加してください。");
				return;
			}
			try {
				const res = await originalFetch("/api/bots/assistant/guild-note", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify({
						botId: window.currentBotId,
						guildId,
						content: noteArea ? noteArea.value : "",
					}),
				});
				const data = await res.json();
				alert(
					data.message ||
						(data.success ? "保存しました。" : "保存に失敗しました。"),
				);
			} catch (e) {
				alert("通信エラーが発生しました。");
			}
		});
	}

	// Bot登録名の変更（/api/bots/profile に name のみ送信。avatar は COALESCE で保持される）
	const botNameForm = document.getElementById("bot-name-form");
	if (botNameForm) {
		botNameForm.addEventListener("submit", async (e) => {
			e.preventDefault();
			const botId = window.currentBotId;
			const name = document.getElementById("bot-name-input").value.trim();
			const fb = document.getElementById("bot-name-feedback");
			const btn = botNameForm.querySelector('button[type="submit"]');
			const showFb = (msg, ok) => {
				if (fb) {
					fb.style.display = "block";
					fb.style.color = ok ? "" : "#f87171";
					fb.textContent = msg;
				}
			};
			if (
				!botId ||
				botId === "system_default" ||
				botId.startsWith("bot_default_")
			)
				return;
			if (!name) {
				showFb("登録名を入力してください。", false);
				return;
			}
			if (btn) btn.disabled = true;
			try {
				const res = await originalFetch("/api/bots/profile", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify({ botId, name }),
				});
				const data = await res.json();
				if (data.success) {
					showFb("登録名を変更しました。", true);
					// Discordの表示名が無いBotは登録名がそのまま表示名になるため、サイドバー表示も更新。
					const discordUsername = botNameForm.dataset.discordUsername || "";
					if (!discordUsername && window.currentBotId === botId) {
						localStorage.setItem("currentBotName", name);
						if (activeBotDisplay) activeBotDisplay.textContent = name;
						updateSidebarBotBranding();
					}
					fetchBotList();
				} else {
					showFb(data.message || "変更に失敗しました。", false);
				}
			} catch (err) {
				showFb("通信エラーが発生しました。", false);
			} finally {
				if (btn) btn.disabled = false;
			}
		});
	}

	// Handle Profile settings update
	const profileConfigForm = document.getElementById("profile-config-form");
	if (profileConfigForm) {
		profileConfigForm.addEventListener("submit", async (e) => {
			e.preventDefault();
			const username = document
				.getElementById("config-profile-username")
				.value.trim();
			try {
				const res = await fetch("/api/settings/profile", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify({ username }),
				});
				const data = await res.json();
				if (data.success) {
					alert("プロフィールを更新しました。");
					initAppSession();
				} else {
					alert(data.message);
				}
			} catch (err) {
				alert("通信エラーが発生しました。");
			}
		});
	}

	// Handle Gemini settings update
	const geminiConfigForm = document.getElementById("gemini-config-form");
	if (geminiConfigForm) {
		geminiConfigForm.addEventListener("submit", async (e) => {
			e.preventDefault();
			const apiKey = document.getElementById("gemini-api-key").value.trim();
			const model = document.getElementById("gemini-model-select").value;
			try {
				const res = await fetch("/api/settings/gemini", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify({ apiKey, model }),
				});
				const data = await res.json();
				if (data.success) {
					alert("Gemini 設定を更新しました。");
					document.getElementById("gemini-api-key").value = "";
					fetchConfigSettings();
				} else {
					alert(data.message);
				}
			} catch (err) {
				alert("通信エラーが発生しました。");
			}
		});
	}

	// Handle Discord token settings update (v2: トークンのみ)
	const discordConfigForm = document.getElementById("discord-config-form");
	if (discordConfigForm) {
		discordConfigForm.addEventListener("submit", async (e) => {
			e.preventDefault();
			const token = document.getElementById("discord-token").value.trim();

			try {
				const res = await fetch("/api/settings/discord", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify({ token }),
				});
				const data = await res.json();
				if (data.success) {
					alert(data.message);
					fetchConfigSettings();
				} else {
					alert(data.message);
				}
			} catch (err) {
				alert("通信エラーが発生しました。");
			}
		});
	}

	// ペルソナ / コンテキストノートへの誘導リンク

	// Handle password change
	const passwordConfigForm = document.getElementById("password-config-form");
	if (passwordConfigForm) {
		passwordConfigForm.addEventListener("submit", async (e) => {
			e.preventDefault();
			const currentPassword = document.getElementById(
				"config-current-password",
			).value;
			const newPassword = document.getElementById("config-new-password").value;
			if (!currentPassword || !newPassword) return;

			try {
				const res = await fetch("/api/settings/password", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify({ currentPassword, newPassword }),
				});
				const data = await res.json();
				alert(
					data.message ||
						(data.success
							? "パスワードを変更しました。"
							: "パスワードの変更に失敗しました。"),
				);
				if (data.success) passwordConfigForm.reset();
			} catch (err) {
				alert("通信エラーが発生しました。");
			}
		});
	}

	// Handle account deletion (self-service withdrawal)
	const deleteAccountForm = document.getElementById("delete-account-form");
	if (deleteAccountForm) {
		deleteAccountForm.addEventListener("submit", async (e) => {
			e.preventDefault();
			const password = document.getElementById("config-delete-password").value;
			if (!password) return;
			if (
				!confirm(
					"本当にアカウントを削除しますか？\n所有するBotや秘書業務データを含むすべての関連データが削除され、この操作は取り消せません。",
				)
			)
				return;

			try {
				const res = await fetch("/api/settings/delete-account", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify({ password }),
				});
				const data = await res.json();
				if (data.success) {
					alert(data.message || "アカウントを削除しました。");
					localStorage.removeItem("currentBotId");
					localStorage.removeItem("currentBotName");
					window.currentBotId = null;
					activeUserId = "";
					navigateTo("/login");
				} else {
					alert(data.message || "アカウントの削除に失敗しました。");
				}
			} catch (err) {
				alert("通信エラーが発生しました。");
			}
		});
	}

	// Handle assistant (user) settings update
	const userSettingsForm = document.getElementById("user-settings-form");
	if (userSettingsForm) {
		userSettingsForm.addEventListener("submit", async (e) => {
			e.preventDefault();
			const richReplyEnabled =
				document.getElementById("user-rich-reply").checked;
			const remindDefaultMinutes =
				parseInt(document.getElementById("user-remind-default").value, 10) || 0;
			const notifyTargetType =
				document.getElementById("user-notify-type").value === "channel"
					? "channel"
					: "dm";
			const notifyTargetId = document
				.getElementById("user-notify-id")
				.value.trim();
			const timezone = document.getElementById("user-timezone").value.trim();

			const payload = {
				richReplyEnabled,
				remindDefaultMinutes,
				notifyTargetType,
				notifyTargetId,
			};
			if (timezone) payload.timezone = timezone;

			try {
				const res = await fetch("/api/settings/user", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify(payload),
				});
				const data = await res.json();
				if (data.success) {
					alert("アシスタント設定を保存しました。");
				} else {
					alert(data.message || "設定の保存に失敗しました。");
				}
			} catch (err) {
				alert("通信エラーが発生しました。");
			}
		});
	}

	// ==========================================
	// BOT SHARING MANAGEMENT (オーナーのみ §5.2)
	// ==========================================
	async function fetchBotShares() {
		const shareCard = document.getElementById("config-share-card");
		const sharesList = document.getElementById("bot-shares-list");
		// 推奨ペルソナカードは「ペルソナ」タブに移設済み。共有カードと同じ owner 限定の可視条件で連動させる。
		const recommendedPersonaCard = document.getElementById(
			"config-recommended-persona-card",
		);
		if (!shareCard || !sharesList) return;

		const hideOwnerCards = () => {
			shareCard.classList.add("hidden");
			if (recommendedPersonaCard)
				recommendedPersonaCard.classList.add("hidden");
		};

		const botId = window.currentBotId;
		if (!botId || botId === "system_default") {
			hideOwnerCards();
			return;
		}

		try {
			const res = await originalFetch(
				`/api/bots/shares?botId=${encodeURIComponent(botId)}`,
			);
			const data = await res.json();
			if (!data.success) {
				// 403 = オーナーではない → カードを隠す
				hideOwnerCards();
				return;
			}
			shareCard.classList.remove("hidden");
			if (recommendedPersonaCard)
				recommendedPersonaCard.classList.remove("hidden");
			renderBotShares(data.shares || []);
			populateRecommendedPersonaSelect(data.recommended_persona_id ?? null);
		} catch (err) {
			hideOwnerCards();
		}
	}

	function renderBotShares(shares) {
		const sharesList = document.getElementById("bot-shares-list");
		sharesList.replaceChildren();

		const visible = shares.filter((s) => s.status !== "revoked");
		if (visible.length === 0) {
			const empty = document.createElement("p");
			empty.style.cssText =
				"font-size:0.8rem;color:var(--text-secondary);margin:0;";
			empty.textContent = "共有中のユーザーはいません。";
			sharesList.appendChild(empty);
			return;
		}

		visible.forEach((share) => {
			const row = document.createElement("div");
			row.style.cssText =
				"display:flex;align-items:center;justify-content:space-between;gap:10px;padding:8px 12px;border:1px solid var(--border-matte);border-radius:var(--radius);";

			const info = document.createElement("div");
			info.style.cssText =
				"display:flex;align-items:center;gap:10px;min-width:0;";

			const nameSpan = document.createElement("span");
			nameSpan.style.cssText =
				"font-size:0.88rem;font-weight:600;color:var(--text-primary);";
			nameSpan.textContent = share.shared_username || share.shared_user_id;

			const badge = document.createElement("span");
			badge.className = `status-badge ${share.status === "active" ? "status-sent" : "status-pending"}`;
			badge.textContent = share.status === "active" ? "承認済み" : "招待中";

			info.appendChild(nameSpan);
			info.appendChild(badge);

			const revokeBtn = document.createElement("button");
			revokeBtn.type = "button";
			revokeBtn.className = "btn btn-secondary btn-sm";
			revokeBtn.style.cssText = "font-size:0.72rem;padding:3px 10px;";
			revokeBtn.textContent = "取り消し";
			revokeBtn.addEventListener("click", async () => {
				if (
					!confirm(
						`${share.shared_username || share.shared_user_id} さんへの共有を取り消しますか？`,
					)
				)
					return;
				try {
					const res = await originalFetch("/api/bots/shares/revoke", {
						method: "POST",
						headers: { "Content-Type": "application/json" },
						body: JSON.stringify({
							botId: window.currentBotId,
							targetUserId: share.shared_user_id,
						}),
					});
					const data = await res.json();
					if (data.success) {
						fetchBotShares();
					} else {
						alert(data.message || "取り消しに失敗しました。");
					}
				} catch (err) {
					alert("通信エラーが発生しました。");
				}
			});

			row.appendChild(info);
			row.appendChild(revokeBtn);
			sharesList.appendChild(row);
		});
	}

	const botShareInviteForm = document.getElementById("bot-share-invite-form");
	if (botShareInviteForm) {
		botShareInviteForm.addEventListener("submit", async (e) => {
			e.preventDefault();
			const targetUserId = document
				.getElementById("share-target-user-id")
				.value.trim();
			if (!targetUserId) return;
			try {
				const res = await originalFetch("/api/bots/shares/invite", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify({ botId: window.currentBotId, targetUserId }),
				});
				const data = await res.json();
				alert(
					data.message ||
						(data.success ? "招待を送信しました。" : "招待に失敗しました。"),
				);
				if (data.success) {
					botShareInviteForm.reset();
					fetchBotShares();
				}
			} catch (err) {
				alert("通信エラーが発生しました。");
			}
		});
	}

	async function populateRecommendedPersonaSelect(recommendedId) {
		const sel = document.getElementById("recommended-persona-select");
		if (!sel) return;
		try {
			const res = await fetch("/api/personas/marketplace");
			const data = await res.json();
			sel.innerHTML = '<option value="">-- 推奨ペルソナなし --</option>';
			if (data.success && data.personas) {
				data.personas.forEach((p) => {
					const opt = document.createElement("option");
					opt.value = p.id;
					opt.textContent = `${p.name} (by ${p.owner_username})`;
					sel.appendChild(opt);
				});
			}
			sel.value = recommendedId != null ? String(recommendedId) : "";
		} catch (err) {
			console.error("公開ペルソナの取得失敗:", err);
		}
	}

	const btnSaveRecommendedPersona = document.getElementById(
		"btn-save-recommended-persona",
	);
	if (btnSaveRecommendedPersona) {
		btnSaveRecommendedPersona.addEventListener("click", async () => {
			const sel = document.getElementById("recommended-persona-select");
			const personaId = sel.value ? Number(sel.value) : null;
			try {
				const res = await originalFetch("/api/bots/recommended-persona", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify({ botId: window.currentBotId, personaId }),
				});
				const data = await res.json();
				alert(
					data.message ||
						(data.success ? "設定しました。" : "設定に失敗しました。"),
				);
			} catch (err) {
				alert("通信エラーが発生しました。");
			}
		});
	}

	// Handle Backup config update
	const backupConfigForm = document.getElementById("backup-config-form");
	if (backupConfigForm) {
		backupConfigForm.addEventListener("submit", async (e) => {
			e.preventDefault();
			const enabled = document.getElementById("backup-enable").checked;
			const folderId = document.getElementById("backup-folder-id").value.trim();
			const intervalHours = Math.min(
				Math.max(
					parseInt(
						document.getElementById("backup-interval-hours").value,
						10,
					) || 24,
					1,
				),
				720,
			);
			const generations = Math.max(
				parseInt(document.getElementById("backup-generations").value, 10) || 7,
				1,
			);

			try {
				const res = await fetch("/api/settings/backup", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify({
						enabled,
						folderId,
						intervalHours,
						generations,
					}),
				});
				const data = await res.json();
				if (data.success) {
					alert("バックアップ設定を保存しました。");
					fetchConfigSettings();
				} else {
					alert(`設定の保存に失敗しました: ${data.message}`);
				}
			} catch (e) {
				console.error(e);
				alert("通信エラーが発生しました。");
			}
		});
	}

	// Handle Manual Backup Trigger
	const btnTriggerBackup = document.getElementById("btn-trigger-backup");
	if (btnTriggerBackup) {
		btnTriggerBackup.addEventListener("click", async () => {
			if (!document.getElementById("backup-enable").checked) {
				alert(
					"手動バックアップを実行するには、先にバックアップを有効にして設定を保存してください。",
				);
				return;
			}

			const originalText = btnTriggerBackup.textContent;
			btnTriggerBackup.textContent = "バックアップ実行中...";
			btnTriggerBackup.disabled = true;

			try {
				const res = await fetch("/api/settings/backup/trigger", {
					method: "POST",
				});
				const data = await res.json();
				if (data.success) {
					alert(`バックアップが完了しました！\nURL: ${data.url}`);
				} else {
					alert(`バックアップ失敗: ${data.message}`);
				}
			} catch (e) {
				console.error(e);
				alert("バックアップ処理中にエラーが発生しました。");
			} finally {
				btnTriggerBackup.textContent = originalText;
				btnTriggerBackup.disabled = false;
			}
		});
	}

	// ==========================================
	// AI CREDENTIALS MANAGEMENT
	// ==========================================
	async function fetchCredentialsSettings() {
		const configCredentialsList = document.getElementById(
			"config-credentials-list",
		);
		if (!configCredentialsList) return;

		configCredentialsList.replaceChildren();

		try {
			const res = await fetch("/api/credentials");
			const data = await res.json();

			if (data.success && data.credentials.length > 0) {
				data.credentials.forEach((cred) => {
					const tr = document.createElement("tr");
					tr.style.borderBottom = "1px solid var(--border-matte)";

					const serviceName = cred.service_name || cred.serviceName;

					const tdService = document.createElement("td");
					tdService.style.padding = "12px 10px";
					tdService.style.fontSize = "0.85rem";
					tdService.style.color = "var(--color-white)";
					tdService.style.fontWeight = "700";
					tdService.textContent = serviceName;

					const tdUser = document.createElement("td");
					tdUser.style.padding = "12px 10px";
					tdUser.style.fontSize = "0.85rem";
					tdUser.style.fontFamily = "var(--font-family-mono)";
					tdUser.textContent = cred.username;

					const tdPass = document.createElement("td");
					tdPass.style.padding = "12px 10px";
					tdPass.style.fontSize = "0.85rem";
					tdPass.style.color = "var(--color-zinc-muted)";
					tdPass.style.fontFamily = "var(--font-family-mono)";
					tdPass.textContent = "••••••••••••";

					const tdUrl = document.createElement("td");
					tdUrl.style.padding = "12px 10px";
					tdUrl.style.fontSize = "0.78rem";
					tdUrl.style.fontFamily = "var(--font-family-mono)";
					tdUrl.style.maxWidth = "200px";
					tdUrl.style.overflow = "hidden";
					tdUrl.style.textOverflow = "ellipsis";
					tdUrl.style.whiteSpace = "nowrap";
					tdUrl.title = cred.url || "";
					tdUrl.textContent = cred.url || "—";

					const tdDate = document.createElement("td");
					tdDate.style.padding = "12px 10px";
					tdDate.style.fontSize = "0.75rem";
					tdDate.style.color = "var(--color-zinc-muted)";
					tdDate.textContent = cred.updated_at || cred.updatedAt || "";

					tr.appendChild(tdService);
					tr.appendChild(tdUser);
					tr.appendChild(tdPass);
					tr.appendChild(tdUrl);
					tr.appendChild(tdDate);

					configCredentialsList.appendChild(tr);
				});
			} else {
				const tr = document.createElement("tr");
				const td = document.createElement("td");
				td.colSpan = 5;
				td.style.textAlign = "center";
				td.style.padding = "20px";
				td.style.color = "var(--color-zinc-muted)";
				td.style.fontSize = "0.8rem";
				td.textContent = "利用可能なAI認証情報はありません。";
				tr.appendChild(td);
				configCredentialsList.appendChild(tr);
			}
		} catch (err) {
			console.error("認証情報取得失敗:", err);
		}
	}

	credentialForm.addEventListener("submit", async (e) => {
		e.preventDefault();
		const serviceName = document
			.getElementById("cred-service-name")
			.value.trim()
			.toLowerCase();
		const username = document.getElementById("cred-username").value.trim();
		const password = document.getElementById("cred-password").value;
		const url = document.getElementById("cred-url").value.trim();

		try {
			const res = await fetch("/api/credentials/register", {
				method: "POST",
				headers: { "Content-Type": "application/json" },
				body: JSON.stringify({
					serviceName,
					username,
					password,
					...(url ? { url } : {}),
				}),
			});
			const data = await res.json();
			if (data.success) {
				closeModal(modalCredential);
				credentialForm.reset();
				fetchCredentialsSettings();
			} else {
				alert(`登録失敗: ${data.message}`);
			}
		} catch (err) {
			console.error(err);
			alert("通信エラーが発生しました。");
		}
	});

	// ==========================================
	// ADMIN PAGE LOGIC
	// ==========================================

	function maskDiscordId(discordId) {
		if (!discordId || discordId.length < 6) return discordId || "";
		return (
			discordId.substring(0, 4) +
			"****" +
			discordId.substring(discordId.length - 4)
		);
	}

	async function fetchAdminData() {
		if (activeUserRole !== "admin") return;
		await Promise.all([
			fetchAdminStats(),
			fetchAdminUsers(),
			fetchAdminBots(),
			fetchAdminInviteCodes(),
			fetchAdminSystemSettings(),
			fetchAdminAuditLogs(),
			fetchAdminPersonas(),
			fetchAdminBotAttributeSettings(),
		]);
	}

	// ── Bot属性設定（プリセット表示名・レート制限既定値） ──

	async function fetchAdminBotAttributeSettings() {
		try {
			const res = await originalFetch("/api/admin/bot-attribute-settings");
			const data = await res.json();
			if (!data.success) return;
			const byId = {};
			(data.presets || []).forEach((p) => {
				byId[p.id] = p;
			});
			const nameSec = document.getElementById("admin-preset-name-secretary");
			const nameMcp = document.getElementById("admin-preset-name-mcp");
			if (nameSec && byId.secretary) nameSec.value = byId.secretary.displayName;
			if (nameMcp && byId.mcp_assistant)
				nameMcp.value = byId.mcp_assistant.displayName;
			const rl = data.rate_limits || {};
			const userMin = document.getElementById("admin-rate-user-min");
			const userDay = document.getElementById("admin-rate-user-day");
			const guildDay = document.getElementById("admin-rate-guild-day");
			if (userMin) userMin.value = rl.userPerMinute ?? 5;
			if (userDay) userDay.value = rl.userPerDay ?? 100;
			if (guildDay) guildDay.value = rl.guildPerDay ?? 1000;
		} catch (err) {
			console.error("Bot属性設定の取得に失敗しました:", err);
		}
	}

	const adminBotAttrForm = document.getElementById("admin-bot-attr-form");
	if (adminBotAttrForm) {
		adminBotAttrForm.addEventListener("submit", async (e) => {
			e.preventDefault();
			try {
				const res = await originalFetch("/api/admin/bot-attribute-settings", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify({
						displayNames: {
							secretary: document
								.getElementById("admin-preset-name-secretary")
								.value.trim(),
							mcp_assistant: document
								.getElementById("admin-preset-name-mcp")
								.value.trim(),
						},
						rateLimits: {
							userPerMinute: Number(
								document.getElementById("admin-rate-user-min").value,
							),
							userPerDay: Number(
								document.getElementById("admin-rate-user-day").value,
							),
							guildPerDay: Number(
								document.getElementById("admin-rate-guild-day").value,
							),
						},
					}),
				});
				const data = await res.json();
				alert(
					data.message ||
						(data.success ? "保存しました。" : "保存に失敗しました。"),
				);
				if (data.success) fetchAdminBotAttributeSettings();
			} catch (err) {
				alert("通信エラーが発生しました。");
			}
		});
	}

	async function fetchAdminStats() {
		try {
			const res = await originalFetch("/api/admin/stats");
			const data = await res.json();
			if (data.success) {
				const s = data.stats;
				document.getElementById("admin-stat-users").textContent = s.totalUsers;
				document.getElementById("admin-stat-bots").textContent = s.totalBots;
				document.getElementById("admin-stat-suspended").textContent =
					s.suspendedBots;
				document.getElementById("admin-stat-invites").textContent =
					s.availableInviteCodes;
			}
		} catch (err) {
			console.error("Admin stats fetch error");
		}
	}

	async function fetchAdminUsers() {
		const tbody = document.getElementById("admin-users-tbody");
		if (!tbody) return;
		tbody.replaceChildren();

		try {
			const res = await originalFetch("/api/admin/users");
			const data = await res.json();
			if (!data.success || !data.users.length) {
				const tr = document.createElement("tr");
				const td = document.createElement("td");
				td.colSpan = 5;
				td.style.textAlign = "center";
				td.style.padding = "20px";
				td.style.color = "var(--color-zinc-muted)";
				td.style.fontSize = "0.8rem";
				td.textContent = "登録済みユーザーはいません。";
				tr.appendChild(td);
				tbody.appendChild(tr);
				return;
			}

			data.users.forEach((user) => {
				const tr = document.createElement("tr");

				// ユーザー名
				const tdName = document.createElement("td");
				tdName.className = "admin-table-td";
				tdName.style.fontWeight = "700";
				tdName.style.color = "var(--color-white)";
				tdName.textContent = user.username;
				tr.appendChild(tdName);

				// Discord ID (マスク表示)
				const tdId = document.createElement("td");
				tdId.className = "admin-table-td admin-discord-id";
				tdId.textContent = maskDiscordId(user.discord_id);
				tr.appendChild(tdId);

				// ロールバッジ
				const tdRole = document.createElement("td");
				tdRole.className = "admin-table-td";
				const roleBadge = document.createElement("span");
				roleBadge.className = `admin-role-badge role-${user.role || "user"}`;
				roleBadge.textContent = (user.role || "user").toUpperCase();
				tdRole.appendChild(roleBadge);
				tr.appendChild(tdRole);

				// 登録日
				const tdDate = document.createElement("td");
				tdDate.className = "admin-table-td";
				tdDate.style.fontSize = "0.8rem";
				tdDate.style.color = "var(--color-zinc-muted)";
				tdDate.textContent = user.created_at;
				tr.appendChild(tdDate);

				// 操作
				const tdAction = document.createElement("td");
				tdAction.className = "admin-table-td";
				tdAction.style.textAlign = "right";

				if (user.discord_id !== activeUserId) {
					const actionGroup = document.createElement("div");
					actionGroup.className = "admin-action-group";

					const btn = document.createElement("button");
					btn.className = `admin-btn-action ${user.role === "admin" ? "" : "btn-promote"}`;
					btn.type = "button";
					const newRole = user.role === "admin" ? "user" : "admin";
					btn.textContent = user.role === "admin" ? "降格" : "管理者に";
					btn.addEventListener("click", async () => {
						const confirmMsg =
							newRole === "admin"
								? `ユーザー「${user.username}」を Admin に昇格しますか？`
								: `ユーザー「${user.username}」の Admin 権限を解除しますか？`;
						if (!confirm(confirmMsg)) return;
						try {
							const r = await originalFetch("/api/admin/users/role", {
								method: "POST",
								headers: { "Content-Type": "application/json" },
								body: JSON.stringify({
									targetUserId: user.discord_id,
									role: newRole,
								}),
							});
							const d = await r.json();
							if (d.success) {
								fetchAdminUsers();
								fetchAdminStats();
							} else {
								// TODO(security): Replace alert with a modal in production
								alert(d.message);
							}
						} catch (e) {
							console.error("Role change failed");
						}
					});
					actionGroup.appendChild(btn);

					// ユーザー削除ボタン（§5.3.2）
					const btnDelete = document.createElement("button");
					btnDelete.className = "admin-btn-action btn-danger";
					btnDelete.type = "button";
					btnDelete.textContent = "削除";
					btnDelete.addEventListener("click", async () => {
						if (
							!confirm(
								`ユーザー「${user.username}」を完全に削除しますか？\nタスク・家計簿・ペルソナ等の関連データも全て削除され、元に戻せません。`,
							)
						)
							return;
						try {
							const r = await originalFetch("/api/admin/users/delete", {
								method: "POST",
								headers: { "Content-Type": "application/json" },
								body: JSON.stringify({ targetUserId: user.discord_id }),
							});
							const d = await r.json();
							if (d.success) {
								alert(d.message);
								fetchAdminUsers();
								fetchAdminStats();
								fetchAdminBots();
							} else {
								alert(d.message || "削除に失敗しました。");
							}
						} catch (e) {
							console.error("User delete failed");
							alert("削除処理中にエラーが発生しました。");
						}
					});
					actionGroup.appendChild(btnDelete);
					tdAction.appendChild(actionGroup);
				} else {
					const selfLabel = document.createElement("span");
					selfLabel.style.fontSize = "0.75rem";
					selfLabel.style.color = "var(--color-zinc-muted)";
					selfLabel.textContent = "自分";
					tdAction.appendChild(selfLabel);
				}
				tr.appendChild(tdAction);

				tbody.appendChild(tr);
			});
		} catch (err) {
			console.error("Admin users fetch error");
		}
	}

	async function fetchAdminBots() {
		const tbody = document.getElementById("admin-bots-tbody");
		if (!tbody) return;
		tbody.replaceChildren();

		try {
			const res = await originalFetch("/api/admin/bots");
			const data = await res.json();
			if (!data.success || !data.bots.length) {
				const tr = document.createElement("tr");
				const td = document.createElement("td");
				td.colSpan = 5;
				td.style.textAlign = "center";
				td.style.padding = "20px";
				td.style.color = "var(--color-zinc-muted)";
				td.style.fontSize = "0.8rem";
				td.textContent = "Botは登録されていません。";
				tr.appendChild(td);
				tbody.appendChild(tr);
				return;
			}

			data.bots.forEach((bot) => {
				const tr = document.createElement("tr");

				// Bot名
				const tdName = document.createElement("td");
				tdName.className = "admin-table-td";
				tdName.style.fontWeight = "700";
				tdName.style.color = "var(--color-white)";
				tdName.textContent = bot.discord_username || bot.name;
				tr.appendChild(tdName);

				// 所有者
				const tdOwner = document.createElement("td");
				tdOwner.className = "admin-table-td";
				tdOwner.textContent = bot.owner_username;
				tr.appendChild(tdOwner);

				// ステータス
				const tdStatus = document.createElement("td");
				tdStatus.className = "admin-table-td";
				const statusBadge = document.createElement("span");
				if (bot.suspended) {
					statusBadge.className = "admin-status-badge status-suspended";
					statusBadge.textContent = "差し押さえ中";
				} else if (bot.hasCustomToken) {
					statusBadge.className = "admin-status-badge status-active";
					statusBadge.textContent = bot.isRunning ? "起動中" : "停止";
				} else {
					statusBadge.className = "admin-status-badge status-default";
					statusBadge.textContent = "デフォルト";
				}
				tdStatus.appendChild(statusBadge);
				tr.appendChild(tdStatus);

				// 作成日
				const tdDate = document.createElement("td");
				tdDate.className = "admin-table-td";
				tdDate.style.fontSize = "0.8rem";
				tdDate.style.color = "var(--color-zinc-muted)";
				tdDate.textContent = bot.created_at;
				tr.appendChild(tdDate);

				// 操作
				const tdAction = document.createElement("td");
				tdAction.className = "admin-table-td";
				tdAction.style.textAlign = "right";

				if (bot.suspended) {
					const btnUnsuspend = document.createElement("button");
					btnUnsuspend.className = "admin-btn-action btn-success";
					btnUnsuspend.type = "button";
					btnUnsuspend.textContent = "解除";
					btnUnsuspend.addEventListener("click", async () => {
						if (!confirm(`Bot「${bot.name}」の差し押さえを解除しますか？`))
							return;
						try {
							const r = await originalFetch("/api/admin/bots/unsuspend", {
								method: "POST",
								headers: { "Content-Type": "application/json" },
								body: JSON.stringify({ botId: bot.id }),
							});
							const d = await r.json();
							if (d.success) {
								fetchAdminBots();
								fetchAdminStats();
							}
						} catch (e) {
							console.error("Unsuspend failed");
						}
					});
					tdAction.appendChild(btnUnsuspend);
				} else {
					const btnSuspend = document.createElement("button");
					btnSuspend.className = "admin-btn-action btn-danger";
					btnSuspend.type = "button";
					btnSuspend.textContent = "差し押さえ";
					btnSuspend.addEventListener("click", async () => {
						if (
							!confirm(
								`Bot「${bot.name}」を差し押さえますか？\nDiscordクライアントが停止され、所有者は再起動できなくなります。`,
							)
						)
							return;
						try {
							const r = await originalFetch("/api/admin/bots/suspend", {
								method: "POST",
								headers: { "Content-Type": "application/json" },
								body: JSON.stringify({ botId: bot.id }),
							});
							const d = await r.json();
							if (d.success) {
								fetchAdminBots();
								fetchAdminStats();
							}
						} catch (e) {
							console.error("Suspend failed");
						}
					});
					tdAction.appendChild(btnSuspend);
				}
				tr.appendChild(tdAction);

				tbody.appendChild(tr);
			});
		} catch (err) {
			console.error("Admin bots fetch error");
		}
	}

	async function fetchAdminInviteCodes() {
		const tbody = document.getElementById("admin-invites-tbody");
		if (!tbody) return;
		tbody.replaceChildren();

		try {
			const res = await originalFetch("/api/admin/invite-codes");
			const data = await res.json();
			if (!data.success || !data.codes.length) {
				const tr = document.createElement("tr");
				const td = document.createElement("td");
				td.colSpan = 5;
				td.style.textAlign = "center";
				td.style.padding = "20px";
				td.style.color = "var(--color-zinc-muted)";
				td.style.fontSize = "0.8rem";
				td.textContent = "招待コードは登録されていません。";
				tr.appendChild(td);
				tbody.appendChild(tr);
				return;
			}

			data.codes.forEach((code) => {
				const tr = document.createElement("tr");

				// コード
				const tdCode = document.createElement("td");
				tdCode.className = "admin-table-td";
				tdCode.style.fontFamily = "var(--font-family-mono)";
				tdCode.style.fontWeight = "600";
				tdCode.textContent = code.code;
				tr.appendChild(tdCode);

				// ステータス
				const tdStatus = document.createElement("td");
				tdStatus.className = "admin-table-td";
				const statusSpan = document.createElement("span");
				if (code.used_by) {
					statusSpan.className = "invite-status-used";
					statusSpan.textContent = "使用済み";
				} else if (code.revoked_at) {
					statusSpan.className = "invite-status-revoked";
					statusSpan.textContent = "無効";
				} else {
					statusSpan.className = "invite-status-available";
					statusSpan.textContent = "利用可能";
				}
				tdStatus.appendChild(statusSpan);
				tr.appendChild(tdStatus);

				// 使用者
				const tdUsedBy = document.createElement("td");
				tdUsedBy.className = "admin-table-td";
				tdUsedBy.style.color = "var(--color-zinc-muted)";
				tdUsedBy.textContent = code.used_by ? maskDiscordId(code.used_by) : "—";
				tr.appendChild(tdUsedBy);

				// 作成日
				const tdDate = document.createElement("td");
				tdDate.className = "admin-table-td";
				tdDate.style.fontSize = "0.8rem";
				tdDate.style.color = "var(--color-zinc-muted)";
				tdDate.textContent = code.created_at;
				tr.appendChild(tdDate);

				// 操作（未使用コードのみ無効化／削除可）
				const tdAction = document.createElement("td");
				tdAction.className = "admin-table-td";
				if (!code.used_by) {
					if (!code.revoked_at) {
						const btnRevoke = document.createElement("button");
						btnRevoke.className = "admin-btn-action";
						btnRevoke.type = "button";
						btnRevoke.textContent = "無効化";
						btnRevoke.addEventListener("click", async () => {
							if (
								!confirm(
									`招待コード「${code.code}」を無効化しますか？\n記録は残りますが、登録には使用できなくなります。`,
								)
							)
								return;
							try {
								const r = await originalFetch(
									`/api/admin/invite-codes/${encodeURIComponent(code.code)}/revoke`,
									{ method: "POST" },
								);
								const d = await r.json();
								if (d.success) {
									fetchAdminInviteCodes();
									fetchAdminStats();
								} else alert(d.message || "無効化に失敗しました。");
							} catch (e) {
								console.error("Invite code revoke failed");
								alert("無効化処理中にエラーが発生しました。");
							}
						});
						tdAction.appendChild(btnRevoke);
					}

					const btnDelete = document.createElement("button");
					btnDelete.className = "admin-btn-action btn-danger";
					btnDelete.type = "button";
					btnDelete.style.marginLeft = "6px";
					btnDelete.textContent = "削除";
					btnDelete.addEventListener("click", async () => {
						if (
							!confirm(
								`招待コード「${code.code}」を完全に削除しますか？\nこの操作は元に戻せません。`,
							)
						)
							return;
						try {
							const r = await originalFetch(
								`/api/admin/invite-codes/${encodeURIComponent(code.code)}`,
								{ method: "DELETE" },
							);
							const d = await r.json();
							if (d.success) {
								fetchAdminInviteCodes();
								fetchAdminStats();
							} else alert(d.message || "削除に失敗しました。");
						} catch (e) {
							console.error("Invite code delete failed");
							alert("削除処理中にエラーが発生しました。");
						}
					});
					tdAction.appendChild(btnDelete);
				} else {
					const dash = document.createElement("span");
					dash.style.color = "var(--color-zinc-muted)";
					dash.textContent = "—";
					tdAction.appendChild(dash);
				}
				tr.appendChild(tdAction);

				tbody.appendChild(tr);
			});
		} catch (err) {
			console.error("Admin invite codes fetch error");
		}
	}

	async function fetchAdminSystemSettings() {
		try {
			const res = await originalFetch("/api/admin/system-settings");
			const data = await res.json();
			if (data.success) {
				const inputPrivacy = document.getElementById(
					"admin-privacy-policy-url",
				);
				if (inputPrivacy) {
					inputPrivacy.value = data.privacyPolicyUrl || "";
				}
				const inputTerms = document.getElementById("admin-terms-url");
				if (inputTerms) {
					inputTerms.value = data.termsUrl || "";
				}
			}
		} catch (err) {
			console.error("Admin system settings fetch error", err);
		}
	}

	// Admin: 監査ログ（§5.3.2）
	const AUDIT_PREVIEW_LIMIT = 5; // インラインで表示する直近件数
	const AUDIT_PAGE_SIZE = 20; // モーダルの1ページあたり件数
	let auditModalPage = 0; // 0始まりのページ番号
	let auditModalAction = ""; // モーダルで適用中のフィルタ

	// 監査ログ1件分の行を生成（インライン・モーダル共通）
	function buildAuditRow(log) {
		const tr = document.createElement("tr");

		const tdDate = document.createElement("td");
		tdDate.className = "admin-table-td";
		tdDate.style.fontSize = "0.78rem";
		tdDate.style.whiteSpace = "nowrap";
		tdDate.style.color = "var(--color-zinc-muted)";
		tdDate.textContent = log.created_at;
		tr.appendChild(tdDate);

		const tdUser = document.createElement("td");
		tdUser.className = "admin-table-td admin-discord-id";
		tdUser.textContent = maskDiscordId(log.user_id);
		tr.appendChild(tdUser);

		const tdAction = document.createElement("td");
		tdAction.className = "admin-table-td";
		tdAction.style.fontFamily = "var(--font-family-mono)";
		tdAction.style.fontSize = "0.78rem";
		tdAction.textContent = log.action;
		tr.appendChild(tdAction);

		const tdTarget = document.createElement("td");
		tdTarget.className = "admin-table-td";
		tdTarget.style.fontSize = "0.78rem";
		tdTarget.style.color = "var(--color-zinc-muted)";
		tdTarget.textContent = log.target || "—";
		tr.appendChild(tdTarget);

		const tdDetail = document.createElement("td");
		tdDetail.className = "admin-table-td";
		tdDetail.style.fontSize = "0.78rem";
		tdDetail.style.color = "var(--color-zinc-muted)";
		tdDetail.textContent = log.detail || "—";
		tr.appendChild(tdDetail);

		return tr;
	}

	function renderAuditEmptyRow(tbody) {
		const tr = document.createElement("tr");
		const td = document.createElement("td");
		td.colSpan = 5;
		td.style.textAlign = "center";
		td.style.padding = "20px";
		td.style.color = "var(--color-zinc-muted)";
		td.style.fontSize = "0.8rem";
		td.textContent = "監査ログはありません。";
		tr.appendChild(td);
		tbody.appendChild(tr);
	}

	// インラインの直近プレビュー
	async function fetchAdminAuditLogs() {
		const tbody = document.getElementById("admin-audit-tbody");
		if (!tbody) return;
		tbody.replaceChildren();

		try {
			const res = await originalFetch(
				`/api/admin/audit-logs?limit=${AUDIT_PREVIEW_LIMIT}`,
			);
			const data = await res.json();
			if (!data.success || !data.logs || !data.logs.length) {
				renderAuditEmptyRow(tbody);
				return;
			}
			data.logs.forEach((log) => tbody.appendChild(buildAuditRow(log)));
		} catch (err) {
			console.error("Admin audit logs fetch error", err);
		}
	}

	// モーダルの全件ページネーション表示
	async function fetchAdminAuditLogsModal() {
		const tbody = document.getElementById("admin-audit-modal-tbody");
		if (!tbody) return;
		tbody.replaceChildren();

		const pageInfo = document.getElementById("admin-audit-page-info");
		const btnPrev = document.getElementById("btn-audit-prev");
		const btnNext = document.getElementById("btn-audit-next");

		try {
			const offset = auditModalPage * AUDIT_PAGE_SIZE;
			const url = `/api/admin/audit-logs?limit=${AUDIT_PAGE_SIZE}&offset=${offset}${auditModalAction ? `&action=${encodeURIComponent(auditModalAction)}` : ""}`;
			const res = await originalFetch(url);
			const data = await res.json();

			if (!data.success || !data.logs || !data.logs.length) {
				renderAuditEmptyRow(tbody);
				if (pageInfo) pageInfo.textContent = "0 件";
				if (btnPrev) btnPrev.disabled = auditModalPage === 0;
				if (btnNext) btnNext.disabled = true;
				return;
			}

			data.logs.forEach((log) => tbody.appendChild(buildAuditRow(log)));

			const total = typeof data.total === "number" ? data.total : 0;
			const totalPages = Math.max(1, Math.ceil(total / AUDIT_PAGE_SIZE));
			const from = offset + 1;
			const to = offset + data.logs.length;
			if (pageInfo)
				pageInfo.textContent = `${from}–${to} 件 / 全 ${total} 件（${auditModalPage + 1} / ${totalPages} ページ）`;
			if (btnPrev) btnPrev.disabled = auditModalPage === 0;
			if (btnNext) btnNext.disabled = auditModalPage + 1 >= totalPages;
		} catch (err) {
			console.error("Admin audit logs modal fetch error", err);
		}
	}

	const adminAuditFilterForm = document.getElementById(
		"admin-audit-filter-form",
	);
	if (adminAuditFilterForm) {
		adminAuditFilterForm.addEventListener("submit", (e) => {
			e.preventDefault();
			const actionInput = document.getElementById("admin-audit-action-input");
			auditModalAction = actionInput ? actionInput.value.trim() : "";
			auditModalPage = 0;
			fetchAdminAuditLogsModal();
		});
	}

	const btnOpenAuditModal = document.getElementById("btn-open-audit-modal");
	const modalAudit = document.getElementById("modal-audit");
	if (btnOpenAuditModal && modalAudit) {
		btnOpenAuditModal.addEventListener("click", () => {
			auditModalPage = 0;
			const actionInput = document.getElementById("admin-audit-action-input");
			auditModalAction = actionInput ? actionInput.value.trim() : "";
			openModal(modalAudit);
			fetchAdminAuditLogsModal();
		});
	}

	const btnAuditPrev = document.getElementById("btn-audit-prev");
	if (btnAuditPrev) {
		btnAuditPrev.addEventListener("click", () => {
			if (auditModalPage > 0) {
				auditModalPage -= 1;
				fetchAdminAuditLogsModal();
			}
		});
	}

	const btnAuditNext = document.getElementById("btn-audit-next");
	if (btnAuditNext) {
		btnAuditNext.addEventListener("click", () => {
			auditModalPage += 1;
			fetchAdminAuditLogsModal();
		});
	}

	// Admin: ペルソナ マーケットプレイス管理（§5.3.2）
	async function fetchAdminPersonas() {
		const tbody = document.getElementById("admin-personas-tbody");
		if (!tbody) return;
		tbody.replaceChildren();

		try {
			const res = await originalFetch("/api/personas/marketplace");
			const data = await res.json();
			if (!data.success || !data.personas || !data.personas.length) {
				const tr = document.createElement("tr");
				const td = document.createElement("td");
				td.colSpan = 5;
				td.style.textAlign = "center";
				td.style.padding = "20px";
				td.style.color = "var(--color-zinc-muted)";
				td.style.fontSize = "0.8rem";
				td.textContent = "公開中のペルソナはありません。";
				tr.appendChild(td);
				tbody.appendChild(tr);
				return;
			}

			data.personas.forEach((p) => {
				const tr = document.createElement("tr");

				const tdId = document.createElement("td");
				tdId.className = "admin-table-td";
				tdId.style.fontFamily = "var(--font-family-mono)";
				tdId.textContent = p.id;
				tr.appendChild(tdId);

				const tdName = document.createElement("td");
				tdName.className = "admin-table-td";
				tdName.style.fontWeight = "700";
				tdName.style.color = "var(--color-white)";
				tdName.textContent = p.name;
				tr.appendChild(tdName);

				const tdOwner = document.createElement("td");
				tdOwner.className = "admin-table-td";
				tdOwner.textContent = p.owner_username;
				tr.appendChild(tdOwner);

				const tdLen = document.createElement("td");
				tdLen.className = "admin-table-td";
				tdLen.style.fontFamily = "var(--font-family-mono)";
				tdLen.textContent = (p.prompt_length || 0).toLocaleString();
				tr.appendChild(tdLen);

				const tdAction = document.createElement("td");
				tdAction.className = "admin-table-td";
				tdAction.style.textAlign = "right";

				const btnUnpublish = document.createElement("button");
				btnUnpublish.className = "admin-btn-action";
				btnUnpublish.type = "button";
				btnUnpublish.textContent = "非公開化";
				btnUnpublish.addEventListener("click", async () => {
					if (!confirm(`ペルソナ「${p.name}」を非公開化しますか？`)) return;
					try {
						const r = await originalFetch("/api/admin/personas/unpublish", {
							method: "POST",
							headers: { "Content-Type": "application/json" },
							body: JSON.stringify({ id: p.id }),
						});
						const d = await r.json();
						if (d.success) {
							fetchAdminPersonas();
						} else {
							alert(d.message || "非公開化に失敗しました。");
						}
					} catch (e) {
						console.error("Persona unpublish failed");
					}
				});
				tdAction.appendChild(btnUnpublish);

				const btnDelete = document.createElement("button");
				btnDelete.className = "admin-btn-action btn-danger";
				btnDelete.type = "button";
				btnDelete.style.marginLeft = "6px";
				btnDelete.textContent = "削除";
				btnDelete.addEventListener("click", async () => {
					if (!confirm(`ペルソナ「${p.name}」を完全に削除しますか？`)) return;
					try {
						const r = await originalFetch("/api/admin/personas/delete", {
							method: "POST",
							headers: { "Content-Type": "application/json" },
							body: JSON.stringify({ id: p.id }),
						});
						const d = await r.json();
						if (d.success) {
							fetchAdminPersonas();
						} else {
							alert(d.message || "削除に失敗しました。");
						}
					} catch (e) {
						console.error("Persona delete failed");
					}
				});
				tdAction.appendChild(btnDelete);

				tr.appendChild(tdAction);
				tbody.appendChild(tr);
			});
		} catch (err) {
			console.error("Admin personas fetch error", err);
		}
	}

	// Admin Invite Code Form
	const adminInviteForm = document.getElementById("admin-invite-form");
	if (adminInviteForm) {
		adminInviteForm.addEventListener("submit", async (e) => {
			e.preventDefault();
			const input = document.getElementById("admin-invite-code-input");
			const code = input.value.trim();
			if (!code) return;

			try {
				const res = await originalFetch("/api/admin/invite-codes", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify({ code }),
				});
				const data = await res.json();
				if (data.success) {
					input.value = "";
					fetchAdminInviteCodes();
					fetchAdminStats();
				} else {
					// TODO(security): Replace alert with a modal in production
					alert(data.message);
				}
			} catch (err) {
				console.error("Invite code creation failed");
			}
		});
	}

	// Admin Default Bot Token Form
	const adminDefaultBotForm = document.getElementById("admin-default-bot-form");
	if (adminDefaultBotForm) {
		adminDefaultBotForm.addEventListener("submit", async (e) => {
			e.preventDefault();
			const input = document.getElementById("admin-default-bot-token");
			const token = input.value.trim();
			if (!token) return;

			try {
				const res = await originalFetch("/api/admin/default-bot/token", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify({ token }),
				});
				const data = await res.json();
				if (data.success) {
					input.value = "";
					alert("システムデフォルト Bot のトークンを更新しました！");
				} else {
					alert(`更新失敗: ${data.message}`);
				}
			} catch (err) {
				console.error("Default bot token update failed");
				alert("更新に失敗しました。");
			}
		});
	}

	// Admin System Settings Form
	const adminSystemSettingsForm = document.getElementById(
		"admin-system-settings-form",
	);
	if (adminSystemSettingsForm) {
		adminSystemSettingsForm.addEventListener("submit", async (e) => {
			e.preventDefault();
			const privacyPolicyUrl = document
				.getElementById("admin-privacy-policy-url")
				.value.trim();
			const termsUrl = document.getElementById("admin-terms-url").value.trim();

			try {
				const res = await originalFetch("/api/admin/system-settings", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify({ privacyPolicyUrl, termsUrl }),
				});
				const data = await res.json();
				if (data.success) {
					alert("システム設定を保存しました。");
					updatePrivacyPolicyLinks(privacyPolicyUrl);
					updateTermsLinks(termsUrl);
				} else {
					alert("保存に失敗しました: " + data.message);
				}
			} catch (err) {
				console.error("System settings save failed", err);
				alert("保存処理中に通信エラーが発生しました。");
			}
		});
	}

	// ==========================================
	// PLAYBOOKS MANAGEMENT
	// ==========================================
	async function fetchPlaybooksList() {
		const container = document.getElementById("playbooks-list-container");
		if (!container) return;

		container.replaceChildren();

		const searchInput = document.getElementById("playbook-search-input");
		const query = searchInput ? searchInput.value.trim() : "";

		try {
			const res = await fetch(
				`/api/playbooks?query=${encodeURIComponent(query)}`,
			);
			const data = await res.json();

			if (data.success && data.playbooks.length > 0) {
				data.playbooks.forEach((pb) => {
					const card = document.createElement("div");
					card.className = "card-item glass hover-lift";
					card.style.flexDirection = "column";
					card.style.alignItems = "stretch";
					card.style.padding = "16px";
					card.style.gap = "8px";
					card.style.cursor = "pointer";

					// Top row: Title and Delete button
					const topRow = document.createElement("div");
					topRow.style.display = "flex";
					topRow.style.justifyContent = "space-between";
					topRow.style.alignItems = "center";

					const title = document.createElement("div");
					title.className = "card-title";
					title.textContent = pb.title;
					title.style.fontSize = "1.0rem";

					const nameTag = document.createElement("span");
					nameTag.className = "badge badge-accent";
					nameTag.style.marginLeft = "8px";
					nameTag.style.fontSize = "0.7rem";
					nameTag.textContent = pb.name;
					title.appendChild(nameTag);

					const btnTrash = document.createElement("button");
					btnTrash.className = "btn-trash";
					btnTrash.type = "button";
					btnTrash.style.width = "28px";
					btnTrash.style.height = "28px";

					const trashIcon = document.createElement("span");
					trashIcon.className = "material-symbols-outlined";
					trashIcon.style.fontSize = "1.1rem";
					trashIcon.textContent = "delete";
					btnTrash.appendChild(trashIcon);

					// Stop propagation so clicking delete doesn't load into edit form
					btnTrash.addEventListener("click", (e) => {
						e.stopPropagation();
						handleDeletePlaybook(pb.name);
					});

					topRow.appendChild(title);
					topRow.appendChild(btnTrash);

					// Middle row: Description
					const desc = document.createElement("div");
					desc.className = "card-desc";
					desc.textContent = pb.description || "説明なし";

					// Keywords list
					const keywordsRow = document.createElement("div");
					keywordsRow.style.display = "flex";
					keywordsRow.style.flexWrap = "wrap";
					keywordsRow.style.gap = "6px";
					keywordsRow.style.marginTop = "4px";

					if (pb.keywords && pb.keywords.length > 0) {
						pb.keywords.forEach((kw) => {
							const kwBadge = document.createElement("span");
							kwBadge.className = "badge";
							kwBadge.style.backgroundColor = "rgba(255,255,255,0.06)";
							kwBadge.style.border = "1px solid var(--border-matte)";
							kwBadge.style.color = "var(--color-zinc-muted)";
							kwBadge.style.fontSize = "0.65rem";
							kwBadge.textContent = kw;
							keywordsRow.appendChild(kwBadge);
						});
					}

					card.appendChild(topRow);
					card.appendChild(desc);
					card.appendChild(keywordsRow);

					// Clicking card loads it into the editor form
					card.addEventListener("click", () => {
						document.getElementById("playbook-name").value = pb.name;
						document.getElementById("playbook-title").value = pb.title;
						document.getElementById("playbook-keywords").value = (
							pb.keywords || []
						).join(", ");
						document.getElementById("playbook-description").value =
							pb.description || "";
						document.getElementById("playbook-steps").value = pb.steps || "";

						// Highlight name field briefly to show it is loaded
						const nameInput = document.getElementById("playbook-name");
						nameInput.style.borderColor = "var(--color-primary)";
						setTimeout(() => {
							nameInput.style.borderColor = "";
						}, 1000);
					});

					container.appendChild(card);
				});
			} else {
				const empty = document.createElement("div");
				empty.className = "glass";
				empty.style.padding = "20px";
				empty.style.textAlign = "center";
				empty.style.color = "var(--color-zinc-muted)";
				empty.style.fontSize = "0.85rem";
				empty.textContent = "Playbookが登録されていません。";
				container.appendChild(empty);
			}
		} catch (e) {
			console.error("Playbook一覧の取得失敗:", e);
		}
	}

	// Handle Playbook form submit
	const playbookForm = document.getElementById("playbook-form");
	if (playbookForm) {
		playbookForm.addEventListener("submit", async (e) => {
			e.preventDefault();
			const name = document.getElementById("playbook-name").value.trim();
			const title = document.getElementById("playbook-title").value.trim();
			const keywordsRaw = document
				.getElementById("playbook-keywords")
				.value.trim();
			const description = document
				.getElementById("playbook-description")
				.value.trim();
			const steps = document.getElementById("playbook-steps").value.trim();

			const keywords = keywordsRaw
				? keywordsRaw
						.split(",")
						.map((k) => k.trim())
						.filter((k) => k.length > 0)
				: [];

			try {
				const res = await fetch("/api/playbooks/save", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify({ name, title, keywords, description, steps }),
				});
				const data = await res.json();
				if (data.success) {
					alert("Playbookを保存しました。");
					playbookForm.reset();
					fetchPlaybooksList();
				} else {
					alert(`保存に失敗しました: ${data.message}`);
				}
			} catch (err) {
				console.error(err);
				alert("通信エラーが発生しました。");
			}
		});
	}

	// Handle Playbook deletion
	async function handleDeletePlaybook(name) {
		if (!confirm(`本当にこのPlaybook [${name}] を削除しますか？`)) return;
		try {
			const res = await fetch("/api/playbooks/delete", {
				method: "POST",
				headers: { "Content-Type": "application/json" },
				body: JSON.stringify({ name }),
			});
			const data = await res.json();
			if (data.success) {
				fetchPlaybooksList();
			} else {
				alert(`削除に失敗しました: ${data.message}`);
			}
		} catch (err) {
			console.error(err);
			alert("通信エラーが発生しました。");
		}
	}

	// Handle search triggers
	const btnPlaybookSearch = document.getElementById("btn-playbook-search");
	if (btnPlaybookSearch) {
		btnPlaybookSearch.addEventListener("click", fetchPlaybooksList);
	}
	const playbookSearchInput = document.getElementById("playbook-search-input");
	if (playbookSearchInput) {
		playbookSearchInput.addEventListener("keydown", (e) => {
			if (e.key === "Enter") {
				fetchPlaybooksList();
			}
		});
	}

	// ── Playbook スケジュール管理 ──────────────────────────

	// Cronプリセット選択 → cron式入力欄に反映
	const schedCronPreset = document.getElementById("schedule-cron-preset");
	const schedCronExpr = document.getElementById("schedule-cron-expression");
	if (schedCronPreset && schedCronExpr) {
		schedCronPreset.addEventListener("change", () => {
			const val = schedCronPreset.value;
			if (val && val !== "custom") {
				schedCronExpr.value = val;
			}
		});
	}

	async function fetchPlaybookSchedulesList() {
		const container = document.getElementById("playbook-schedules-list");
		if (!container) return;
		try {
			const res = await fetch("/api/playbooks/schedules");
			const data = await res.json();
			container.innerHTML = "";
			if (!data.success || !data.schedules || data.schedules.length === 0) {
				const empty = document.createElement("p");
				empty.className = "empty-state-text";
				empty.textContent = "スケジュールが登録されていません。";
				container.appendChild(empty);

				// セレクトボックスもPlaybook一覧で更新
				await _populatePlaybookSelect();
				return;
			}
			data.schedules.forEach((sc) => {
				const card = document.createElement("div");
				card.style.cssText =
					"background: var(--surface-2); border-radius: 8px; padding: 12px; border: 1px solid var(--border-color);";

				const header = document.createElement("div");
				header.style.cssText =
					"display: flex; justify-content: space-between; align-items: center; gap: 8px;";

				const info = document.createElement("div");
				info.style.cssText = "flex: 1; min-width: 0;";
				info.innerHTML = `
          <div style="font-weight: 600; font-size: 0.9rem; truncate; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">${escapeHtml(sc.playbook_name)}</div>
          <div style="font-family: var(--font-family-mono); font-size: 0.78rem; color: var(--text-muted); margin-top: 2px;">${escapeHtml(sc.cron_expression)}</div>
          ${sc.description ? `<div style="font-size: 0.8rem; color: var(--text-secondary); margin-top: 2px;">${escapeHtml(sc.description)}</div>` : ""}
          ${sc.last_run_at ? `<div style="font-size: 0.75rem; color: var(--text-muted); margin-top: 4px;">最終実行: ${escapeHtml(sc.last_run_at)}</div>` : ""}
        `;

				const actions = document.createElement("div");
				actions.style.cssText =
					"display: flex; gap: 6px; align-items: center; flex-shrink: 0;";

				// 有効/無効トグル
				const toggleBtn = document.createElement("button");
				toggleBtn.className = `btn ${sc.enabled ? "btn-secondary" : "btn-primary"}`;
				toggleBtn.style.cssText = "font-size: 0.75rem; padding: 4px 10px;";
				toggleBtn.textContent = sc.enabled ? "無効化" : "有効化";
				toggleBtn.addEventListener("click", async () => {
					await handleToggleSchedule(sc.id, !sc.enabled);
				});

				// 削除ボタン
				const delBtn = document.createElement("button");
				delBtn.className = "btn btn-danger";
				delBtn.style.cssText = "font-size: 0.75rem; padding: 4px 10px;";
				delBtn.textContent = "削除";
				delBtn.addEventListener("click", async () => {
					if (!confirm(`スケジュール「${sc.playbook_name}」を削除しますか？`))
						return;
					await handleDeletePlaybookSchedule(sc.id);
				});

				// ステータスバッジ
				const badge = document.createElement("span");
				badge.style.cssText = `display: inline-block; padding: 2px 8px; border-radius: 12px; font-size: 0.72rem; font-weight: 600; background: ${sc.enabled ? "var(--color-success, #22c55e)" : "var(--border-color)"}; color: ${sc.enabled ? "#fff" : "var(--text-muted)"};`;
				badge.textContent = sc.enabled ? "有効" : "停止中";

				actions.appendChild(badge);
				actions.appendChild(toggleBtn);
				actions.appendChild(delBtn);
				header.appendChild(info);
				header.appendChild(actions);
				card.appendChild(header);
				container.appendChild(card);
			});

			await _populatePlaybookSelect();
		} catch (e) {
			console.error("スケジュール一覧の取得失敗:", e);
		}
	}

	async function _populatePlaybookSelect() {
		const sel = document.getElementById("schedule-playbook-select");
		if (!sel) return;
		try {
			const res = await fetch("/api/playbooks");
			const data = await res.json();
			const current = sel.value;
			sel.innerHTML = '<option value="">-- Playbookを選択 --</option>';
			if (data.success && data.playbooks) {
				data.playbooks.forEach((pb) => {
					const opt = document.createElement("option");
					opt.value = pb.name;
					opt.textContent = `${pb.title} (${pb.name})`;
					sel.appendChild(opt);
				});
			}
			if (current) sel.value = current;
		} catch (e) {
			console.error("Playbook選択肢の取得失敗:", e);
		}
	}

	// 実行ステータスごとのアイコンと色
	function runStatusVisual(status) {
		if (status === "success")
			return { icon: "check_circle", color: "var(--color-success, #22c55e)" };
		if (status === "failed")
			return { icon: "cancel", color: "var(--color-danger, #ef4444)" };
		return { icon: "pending", color: "var(--color-warning, #f59e0b)" };
	}

	// 開始〜終了の所要時間を「N秒」/「N分N秒」形式に整形（未完了なら空文字）
	function formatRunDuration(startedAt, finishedAt) {
		if (!startedAt || !finishedAt) return "";
		const diff = Math.round(
			(new Date(finishedAt) - new Date(startedAt)) / 1000,
		);
		return diff < 60 ? `${diff}秒` : `${Math.floor(diff / 60)}分${diff % 60}秒`;
	}

	async function fetchPlaybookRunsList(scheduleId) {
		const container = document.getElementById("playbook-runs-list");
		if (!container) return;
		try {
			const url = scheduleId
				? `/api/playbooks/runs?scheduleId=${scheduleId}`
				: "/api/playbooks/runs";
			const res = await fetch(url);
			const data = await res.json();
			container.innerHTML = "";
			if (!data.success || !data.runs || data.runs.length === 0) {
				const empty = document.createElement("p");
				empty.className = "empty-state-text";
				empty.textContent = "実行履歴がありません。";
				container.appendChild(empty);
				return;
			}
			data.runs.forEach((run) => {
				const { icon: statusIcon, color: statusColor } = runStatusVisual(
					run.status,
				);
				const duration = formatRunDuration(run.started_at, run.finished_at);

				const row = document.createElement("div");
				row.style.cssText =
					"display: flex; gap: 10px; align-items: flex-start; padding: 8px 10px; background: var(--surface-2); border-radius: 6px; border-left: 3px solid " +
					statusColor +
					";";

				row.innerHTML = `
          <span class="material-symbols-outlined" style="font-size: 1.1rem; color: ${statusColor}; flex-shrink: 0; margin-top: 2px;">${statusIcon}</span>
          <div style="flex: 1; min-width: 0;">
            <div style="font-weight: 600; font-size: 0.85rem;">${escapeHtml(run.playbook_name)}</div>
            <div style="font-size: 0.75rem; color: var(--text-muted);">${escapeHtml(run.started_at)}${duration ? " (" + duration + ")" : ""}</div>
            ${run.output ? `<div style="font-size: 0.78rem; margin-top: 4px; color: var(--text-secondary); max-height: 80px; overflow-y: auto; white-space: pre-wrap; word-break: break-all;">${escapeHtml(run.output.substring(0, 300))}${run.output.length > 300 ? "..." : ""}</div>` : ""}
          </div>
        `;
				container.appendChild(row);
			});
		} catch (e) {
			console.error("実行履歴の取得失敗:", e);
		}
	}

	// スケジュール保存フォーム
	const playbookScheduleForm = document.getElementById(
		"playbook-schedule-form",
	);
	if (playbookScheduleForm) {
		playbookScheduleForm.addEventListener("submit", async (e) => {
			e.preventDefault();
			const playbookName = document.getElementById(
				"schedule-playbook-select",
			).value;
			const cronExpression = document
				.getElementById("schedule-cron-expression")
				.value.trim();
			const description = document
				.getElementById("schedule-description")
				.value.trim();
			const enabled = document.getElementById("schedule-enabled").checked;

			if (!playbookName || !cronExpression) {
				alert("Playbookとcron式を入力してください。");
				return;
			}
			try {
				const res = await fetch("/api/playbooks/schedules/save", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify({
						playbookName,
						cronExpression,
						description,
						enabled,
					}),
				});
				const data = await res.json();
				if (data.success) {
					alert("スケジュールを保存しました。");
					playbookScheduleForm.reset();
					document.getElementById("schedule-enabled").checked = true;
					fetchPlaybookSchedulesList();
					fetchPlaybookRunsList();
				} else {
					alert("エラー: " + (data.message || "保存に失敗しました。"));
				}
			} catch (e) {
				console.error("スケジュール保存エラー:", e);
				alert("保存中にエラーが発生しました。");
			}
		});
	}

	async function handleToggleSchedule(id, enabled) {
		try {
			const res = await fetch("/api/playbooks/schedules/toggle", {
				method: "POST",
				headers: { "Content-Type": "application/json" },
				body: JSON.stringify({ id, enabled }),
			});
			const data = await res.json();
			if (data.success) {
				fetchPlaybookSchedulesList();
			} else {
				alert("エラー: " + (data.message || "操作に失敗しました。"));
			}
		} catch (e) {
			console.error("スケジュールトグルエラー:", e);
		}
	}

	async function handleDeletePlaybookSchedule(id) {
		try {
			const res = await fetch("/api/playbooks/schedules/delete", {
				method: "POST",
				headers: { "Content-Type": "application/json" },
				body: JSON.stringify({ id }),
			});
			const data = await res.json();
			if (data.success) {
				fetchPlaybookSchedulesList();
				fetchPlaybookRunsList();
			} else {
				alert("エラー: " + (data.message || "削除に失敗しました。"));
			}
		} catch (e) {
			console.error("スケジュール削除エラー:", e);
		}
	}

	// ==========================================
	// PERSONAS MANAGEMENT (§4.1)
	// ==========================================
	let personaMaxLength = 20000;
	let activePersonaId = null;
	let myPersonasCache = [];

	const modalPersonaEdit = document.getElementById("modal-persona-edit");
	const personaEditForm = document.getElementById("persona-edit-form");
	const personaEditPrompt = document.getElementById("persona-edit-prompt");
	const personaEditCounter = document.getElementById("persona-edit-counter");

	function updatePersonaCounter() {
		if (!personaEditPrompt || !personaEditCounter) return;
		const len = personaEditPrompt.value.length;
		personaEditCounter.textContent = `${len.toLocaleString()} / ${personaMaxLength.toLocaleString()} 文字`;
		personaEditCounter.style.color =
			len > personaMaxLength ? "var(--color-red)" : "";
	}
	if (personaEditPrompt) {
		personaEditPrompt.addEventListener("input", updatePersonaCounter);
	}

	function openPersonaEditModal(persona) {
		document.getElementById("persona-edit-id").value = persona
			? persona.id
			: "";
		document.getElementById("persona-edit-name").value = persona
			? persona.name
			: "";
		personaEditPrompt.value = persona ? persona.prompt : "";
		document.getElementById("persona-edit-modal-title").textContent = persona
			? `ペルソナの編集: ${persona.name}`
			: "ペルソナの作成";
		updatePersonaCounter();
		openModal(modalPersonaEdit);
	}

	const btnNewPersona = document.getElementById("btn-new-persona");
	if (btnNewPersona) {
		btnNewPersona.addEventListener("click", () => openPersonaEditModal(null));
	}
	const btnCancelPersonaEdit = document.getElementById(
		"btn-cancel-persona-edit",
	);
	if (btnCancelPersonaEdit) {
		btnCancelPersonaEdit.addEventListener("click", () =>
			closeModal(modalPersonaEdit),
		);
	}

	if (personaEditForm) {
		personaEditForm.addEventListener("submit", async (e) => {
			e.preventDefault();
			const idVal = document.getElementById("persona-edit-id").value;
			const name = document.getElementById("persona-edit-name").value.trim();
			const prompt = personaEditPrompt.value;
			if (prompt.length > personaMaxLength) {
				alert(
					`ペルソナ定義は最大 ${personaMaxLength.toLocaleString()} 文字までです。`,
				);
				return;
			}
			try {
				const res = await fetch("/api/personas/save", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify({
						...(idVal ? { id: Number(idVal) } : {}),
						name,
						prompt,
					}),
				});
				const data = await res.json();
				if (data.success) {
					closeModal(modalPersonaEdit);
					personaEditForm.reset();
					fetchPersonasList();
				} else {
					alert(data.message || "保存に失敗しました。");
				}
			} catch (err) {
				alert("通信エラーが発生しました。");
			}
		});
	}

	async function fetchPersonasList() {
		const container = document.getElementById("personas-list");
		if (!container) return;
		container.replaceChildren();

		try {
			const res = await fetch("/api/personas");
			const data = await res.json();
			if (!data.success) return;

			personaMaxLength = data.max_length || 20000;
			activePersonaId = data.active_persona_id ?? null;
			myPersonasCache = data.personas || [];

			// 適用中ラベル
			const activeLabel = document.getElementById("active-persona-label");
			if (activeLabel) {
				const active = myPersonasCache.find((p) => p.id === activePersonaId);
				activeLabel.textContent = `適用中: ${active ? active.name : "デフォルトペルソナ"}`;
			}

			if (myPersonasCache.length === 0) {
				const empty = document.createElement("div");
				empty.className = "glass";
				empty.textContent =
					"ペルソナが作成されていません。「＋ ペルソナ作成」から作成できます。";
				container.appendChild(empty);
				return;
			}

			// 適用中ペルソナを一番上に表示（他は元の順序を維持）
			const sortedPersonas = myPersonasCache.slice().sort((a, b) => {
				const aActive = a.id === activePersonaId ? 0 : 1;
				const bActive = b.id === activePersonaId ? 0 : 1;
				return aActive - bActive;
			});

			sortedPersonas.forEach((p) => {
				const card = document.createElement("div");
				card.className = "card-item glass hover-lift";
				card.style.cssText =
					"flex-direction:column;align-items:stretch;gap:8px;padding:16px;";

				const topRow = document.createElement("div");
				topRow.style.cssText =
					"display:flex;justify-content:space-between;align-items:center;gap:8px;flex-wrap:wrap;";

				const titleWrap = document.createElement("div");
				titleWrap.style.cssText =
					"display:flex;align-items:center;gap:8px;flex-wrap:wrap;";

				const title = document.createElement("div");
				title.className = "card-title";
				title.style.fontSize = "1.0rem";
				title.textContent = p.name;
				titleWrap.appendChild(title);

				if (p.id === activePersonaId) {
					const activeBadge = document.createElement("span");
					activeBadge.className = "status-badge status-sent";
					activeBadge.textContent = "適用中";
					titleWrap.appendChild(activeBadge);
				}
				if (p.is_public === 1) {
					const pubBadge = document.createElement("span");
					pubBadge.className = "status-badge status-pending";
					pubBadge.textContent = "公開中";
					titleWrap.appendChild(pubBadge);
				}

				const lenSpan = document.createElement("span");
				lenSpan.style.cssText =
					"font-size:0.72rem;color:var(--color-zinc-muted);font-family:var(--font-family-mono);";
				lenSpan.textContent = `${(p.prompt || "").length.toLocaleString()} 文字`;

				topRow.appendChild(titleWrap);
				topRow.appendChild(lenSpan);

				const preview = document.createElement("div");
				preview.className = "card-desc";
				preview.style.cssText =
					"white-space:nowrap;overflow:hidden;text-overflow:ellipsis;";
				preview.textContent = (p.prompt || "").substring(0, 120);

				const actions = document.createElement("div");
				actions.style.cssText =
					"display:flex;gap:8px;flex-wrap:wrap;margin-top:4px;";

				const mkBtn = (label, cls, onClick) => {
					const b = document.createElement("button");
					b.type = "button";
					b.className = `btn ${cls} btn-sm`;
					b.style.cssText = "font-size:0.75rem;padding:4px 12px;";
					b.textContent = label;
					b.addEventListener("click", onClick);
					return b;
				};

				if (p.id !== activePersonaId) {
					actions.appendChild(
						mkBtn("適用", "btn-primary", async () => {
							await postPersonaAction("/api/personas/activate", { id: p.id });
						}),
					);
				}
				actions.appendChild(
					mkBtn("編集", "btn-secondary", () => openPersonaEditModal(p)),
				);
				actions.appendChild(
					mkBtn(
						p.is_public === 1 ? "非公開にする" : "公開する",
						"btn-secondary",
						async () => {
							if (
								p.is_public !== 1 &&
								!confirm(
									`ペルソナ「${p.name}」をマーケットプレイスに公開しますか？\n全ユーザーが内容を閲覧・インポートできるようになります。`,
								)
							)
								return;
							await postPersonaAction("/api/personas/publish", {
								id: p.id,
								isPublic: p.is_public !== 1,
							});
						},
					),
				);
				actions.appendChild(
					mkBtn("削除", "btn-secondary", async () => {
						if (!confirm(`ペルソナ「${p.name}」を削除しますか？`)) return;
						await postPersonaAction("/api/personas/delete", { id: p.id });
					}),
				);

				card.appendChild(topRow);
				card.appendChild(preview);
				card.appendChild(actions);
				container.appendChild(card);
			});
		} catch (err) {
			console.error("ペルソナ一覧の取得失敗:", err);
		}
	}

	async function postPersonaAction(url, payload) {
		try {
			const res = await fetch(url, {
				method: "POST",
				headers: { "Content-Type": "application/json" },
				body: JSON.stringify(payload),
			});
			const data = await res.json();
			if (!data.success) {
				alert(data.message || "操作に失敗しました。");
			}
			fetchPersonasList();
			fetchPersonaMarketplace();
		} catch (err) {
			alert("通信エラーが発生しました。");
		}
	}

	const btnResetPersona = document.getElementById("btn-reset-persona");
	if (btnResetPersona) {
		btnResetPersona.addEventListener("click", async () => {
			if (!confirm("デフォルトペルソナに戻しますか？")) return;
			await postPersonaAction("/api/personas/activate", { id: null });
		});
	}

	async function fetchPersonaMarketplace() {
		const container = document.getElementById("persona-marketplace-list");
		if (!container) return;
		container.replaceChildren();

		try {
			const res = await fetch("/api/personas/marketplace");
			const data = await res.json();
			if (!data.success || !data.personas || data.personas.length === 0) {
				const empty = document.createElement("div");
				empty.className = "glass";
				empty.textContent = "公開されているペルソナはまだありません。";
				container.appendChild(empty);
				return;
			}

			data.personas.forEach((p) => {
				const card = document.createElement("div");
				card.className = "card-item glass hover-lift";
				card.style.cssText =
					"flex-direction:column;align-items:stretch;gap:8px;padding:16px;";

				const topRow = document.createElement("div");
				topRow.style.cssText =
					"display:flex;justify-content:space-between;align-items:center;gap:8px;flex-wrap:wrap;";

				const title = document.createElement("div");
				title.className = "card-title";
				title.style.fontSize = "1.0rem";
				title.textContent = p.name;

				const ownerSpan = document.createElement("span");
				ownerSpan.style.cssText =
					"font-size:0.75rem;color:var(--color-zinc-muted);";
				ownerSpan.textContent = `by ${p.owner_username} ・ ${(p.prompt_length || 0).toLocaleString()} 文字`;

				topRow.appendChild(title);
				topRow.appendChild(ownerSpan);

				const preview = document.createElement("div");
				preview.className = "card-desc";
				preview.textContent = p.prompt_preview || "";

				const actions = document.createElement("div");
				actions.style.cssText =
					"display:flex;gap:8px;flex-wrap:wrap;margin-top:4px;";

				const viewBtn = document.createElement("button");
				viewBtn.type = "button";
				viewBtn.className = "btn btn-secondary btn-sm";
				viewBtn.style.cssText = "font-size:0.75rem;padding:4px 12px;";
				viewBtn.textContent = "全文表示";
				viewBtn.addEventListener("click", () => openPersonaPreview(p.id));
				actions.appendChild(viewBtn);

				const importBtn = document.createElement("button");
				importBtn.type = "button";
				importBtn.className = "btn btn-primary btn-sm";
				importBtn.style.cssText = "font-size:0.75rem;padding:4px 12px;";
				importBtn.textContent = "インポート";
				importBtn.addEventListener("click", () => importPersonaById(p.id));
				actions.appendChild(importBtn);

				card.appendChild(topRow);
				card.appendChild(preview);
				card.appendChild(actions);
				container.appendChild(card);
			});
		} catch (err) {
			console.error("マーケットプレイスの取得失敗:", err);
		}
	}

	const modalPersonaPreview = document.getElementById("modal-persona-preview");
	let previewPersonaId = null;

	async function openPersonaPreview(id) {
		try {
			const res = await fetch(`/api/personas/marketplace/${id}`);
			const data = await res.json();
			if (!data.success) {
				alert(data.message || "ペルソナの取得に失敗しました。");
				return;
			}
			previewPersonaId = data.persona.id;
			document.getElementById("persona-preview-title").textContent =
				`ペルソナ プレビュー: ${data.persona.name}`;
			document.getElementById("persona-preview-prompt").textContent =
				data.persona.prompt;
			openModal(modalPersonaPreview);
		} catch (err) {
			alert("通信エラーが発生しました。");
		}
	}

	async function importPersonaById(id) {
		try {
			const res = await fetch("/api/personas/import", {
				method: "POST",
				headers: { "Content-Type": "application/json" },
				body: JSON.stringify({ id }),
			});
			const data = await res.json();
			alert(
				data.message ||
					(data.success
						? "インポートしました。"
						: "インポートに失敗しました。"),
			);
			if (data.success) {
				fetchPersonasList();
			}
		} catch (err) {
			alert("通信エラーが発生しました。");
		}
	}

	const btnImportFromPreview = document.getElementById(
		"btn-import-from-preview",
	);
	if (btnImportFromPreview) {
		btnImportFromPreview.addEventListener("click", async () => {
			if (previewPersonaId == null) return;
			await importPersonaById(previewPersonaId);
			closeModal(modalPersonaPreview);
		});
	}

	// ==========================================
	// REMINDERS MANAGEMENT (§3.3)
	// ==========================================
	async function fetchRemindersList() {
		const container = document.getElementById("reminders-list");
		if (!container) return;
		container.replaceChildren();

		const showAll = document.getElementById("reminders-show-all");
		const includeAll = showAll && showAll.checked;

		try {
			const res = await fetch(`/api/reminders${includeAll ? "?all=1" : ""}`);
			const data = await res.json();
			if (!data.success || !data.reminders || data.reminders.length === 0) {
				const empty = document.createElement("div");
				empty.className = "glass";
				empty.style.cssText =
					"padding:16px;text-align:center;color:var(--color-zinc-muted);font-size:0.85rem;";
				empty.textContent = "リマインダーは登録されていません。";
				container.appendChild(empty);
				return;
			}

			data.reminders.forEach((rem) => {
				const card = document.createElement("div");
				card.className = "card-item glass";
				card.style.cssText =
					"align-items:flex-start;gap:10px;padding:12px 14px;";

				const left = document.createElement("div");
				left.style.cssText =
					"flex:1;min-width:0;display:flex;flex-direction:column;gap:4px;";

				const msgRow = document.createElement("div");
				msgRow.style.cssText =
					"display:flex;align-items:center;gap:8px;flex-wrap:wrap;";

				const msg = document.createElement("span");
				msg.className = "card-title";
				msg.style.fontSize = "0.92rem";
				msg.textContent = rem.message;
				msgRow.appendChild(msg);

				const statusBadge = document.createElement("span");
				statusBadge.className = `status-badge status-${rem.status}`;
				statusBadge.textContent =
					rem.status === "pending"
						? "待機中"
						: rem.status === "sent"
							? "送信済み"
							: "キャンセル";
				msgRow.appendChild(statusBadge);

				const metaRow = document.createElement("div");
				metaRow.style.cssText =
					"font-size:0.78rem;color:var(--color-zinc-muted);display:flex;gap:12px;flex-wrap:wrap;";

				const timeSpan = document.createElement("span");
				timeSpan.textContent = `⏰ ${rem.trigger_at}`;
				metaRow.appendChild(timeSpan);

				if (rem.repeat_rule) {
					const repeatSpan = document.createElement("span");
					repeatSpan.style.fontFamily = "var(--font-family-mono)";
					repeatSpan.textContent = `🔁 ${rem.repeat_rule}`;
					metaRow.appendChild(repeatSpan);
				}

				const targetSpan = document.createElement("span");
				targetSpan.textContent =
					rem.target_type === "channel"
						? `📢 チャンネル${rem.target_id ? `: ${rem.target_id}` : ""}`
						: "📩 DM";
				metaRow.appendChild(targetSpan);

				left.appendChild(msgRow);
				left.appendChild(metaRow);
				card.appendChild(left);

				if (rem.status === "pending") {
					const cancelBtn = document.createElement("button");
					cancelBtn.type = "button";
					cancelBtn.className = "btn btn-secondary btn-sm";
					cancelBtn.style.cssText =
						"font-size:0.72rem;padding:3px 10px;flex-shrink:0;";
					cancelBtn.textContent = "キャンセル";
					cancelBtn.addEventListener("click", async () => {
						if (
							!confirm(`リマインダー「${rem.message}」をキャンセルしますか？`)
						)
							return;
						try {
							const r = await fetch("/api/reminders/cancel", {
								method: "POST",
								headers: { "Content-Type": "application/json" },
								body: JSON.stringify({ reminder_id: rem.id }),
							});
							const d = await r.json();
							if (d.success) {
								fetchRemindersList();
							} else {
								alert(d.message || "キャンセルに失敗しました。");
							}
						} catch (err) {
							alert("通信エラーが発生しました。");
						}
					});
					card.appendChild(cancelBtn);
				}

				container.appendChild(card);
			});
		} catch (err) {
			console.error("リマインダー一覧の取得失敗:", err);
		}
	}

	const remindersShowAll = document.getElementById("reminders-show-all");
	if (remindersShowAll) {
		remindersShowAll.addEventListener("change", fetchRemindersList);
	}

	const reminderForm = document.getElementById("reminder-form");
	if (reminderForm) {
		reminderForm.addEventListener("submit", async (e) => {
			e.preventDefault();
			const message = document.getElementById("reminder-message").value.trim();
			const triggerRaw = document.getElementById("reminder-trigger-at").value; // YYYY-MM-DDTHH:MM
			const repeatRule = document
				.getElementById("reminder-repeat-rule")
				.value.trim();
			const targetType = document.getElementById("reminder-target-type").value;
			const targetId = document
				.getElementById("reminder-target-id")
				.value.trim();

			if (!message || !triggerRaw) return;

			const payload = {
				message,
				trigger_at: triggerRaw.replace("T", " "),
				...(repeatRule ? { repeat_rule: repeatRule } : {}),
				...(targetType ? { target_type: targetType } : {}),
				...(targetId ? { target_id: targetId } : {}),
			};

			try {
				const res = await fetch("/api/reminders/add", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify(payload),
				});
				const data = await res.json();
				if (data.success) {
					reminderForm.reset();
					fetchRemindersList();
				} else {
					alert(data.message || "リマインダーの登録に失敗しました。");
				}
			} catch (err) {
				alert("通信エラーが発生しました。");
			}
		});
	}

	// ==========================================
	// PERSONAL: コンテキストノート / クリップボード / 連絡先 (§3.7, §3.10, §3.11)
	// ==========================================
	let contextNoteMaxLength = 10000;
	const contextNoteTextarea = document.getElementById("context-note-textarea");
	const contextNoteCounter = document.getElementById("context-note-counter");

	function updateContextNoteCounter() {
		if (!contextNoteTextarea || !contextNoteCounter) return;
		const len = contextNoteTextarea.value.length;
		contextNoteCounter.textContent = `${len.toLocaleString()} / ${contextNoteMaxLength.toLocaleString()}`;
		contextNoteCounter.style.color =
			len > contextNoteMaxLength ? "var(--color-red)" : "";
	}
	if (contextNoteTextarea) {
		contextNoteTextarea.addEventListener("input", updateContextNoteCounter);
	}

	async function fetchContextNote() {
		if (!contextNoteTextarea) return;
		try {
			const res = await fetch("/api/context-note");
			const data = await res.json();
			if (data.success) {
				contextNoteMaxLength = data.max_length || 10000;
				contextNoteTextarea.value = data.content || "";
				updateContextNoteCounter();
			}
		} catch (err) {
			console.error("コンテキストノートの取得失敗:", err);
		}
	}

	const contextNoteForm = document.getElementById("context-note-form");
	if (contextNoteForm) {
		contextNoteForm.addEventListener("submit", async (e) => {
			e.preventDefault();
			const content = contextNoteTextarea.value;
			if (content.length > contextNoteMaxLength) {
				alert(
					`コンテキストノートは最大 ${contextNoteMaxLength.toLocaleString()} 文字までです。`,
				);
				return;
			}
			try {
				const res = await fetch("/api/context-note", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify({ content }),
				});
				const data = await res.json();
				alert(
					data.message ||
						(data.success ? "保存しました。" : "保存に失敗しました。"),
				);
			} catch (err) {
				alert("通信エラーが発生しました。");
			}
		});
	}

	async function fetchClipboardList() {
		const container = document.getElementById("clipboard-list");
		if (!container) return;
		container.replaceChildren();

		try {
			const res = await fetch("/api/clipboard");
			const data = await res.json();
			if (!data.success || !data.entries || data.entries.length === 0) {
				const empty = document.createElement("p");
				empty.style.cssText =
					"font-size:0.8rem;color:var(--text-secondary);margin:0;";
				empty.textContent = "クリップボードは空です。";
				container.appendChild(empty);
				return;
			}

			data.entries.forEach((entry) => {
				const row = document.createElement("div");
				row.style.cssText =
					"display:flex;align-items:center;justify-content:space-between;gap:10px;padding:8px 12px;border:1px solid var(--border-matte);border-radius:var(--radius);";

				const left = document.createElement("div");
				left.style.cssText = "flex:1;min-width:0;";

				const content = document.createElement("div");
				content.style.cssText =
					"font-size:0.85rem;color:var(--text-primary);word-break:break-all;";
				content.textContent = entry.content;
				left.appendChild(content);

				const expires = document.createElement("div");
				expires.style.cssText =
					"font-size:0.72rem;color:var(--color-zinc-muted);margin-top:2px;";
				expires.textContent = entry.expires_at
					? `期限: ${entry.expires_at}`
					: "無期限";
				left.appendChild(expires);

				const delBtn = document.createElement("button");
				delBtn.type = "button";
				delBtn.className = "btn-trash";
				delBtn.innerHTML = `<span class="material-symbols-outlined">delete</span>`;
				delBtn.addEventListener("click", async () => {
					if (!confirm("このメモを削除しますか？")) return;
					try {
						const r = await fetch("/api/clipboard/delete", {
							method: "POST",
							headers: { "Content-Type": "application/json" },
							body: JSON.stringify({ id: entry.id }),
						});
						const d = await r.json();
						if (d.success) fetchClipboardList();
					} catch (err) {
						console.error(err);
					}
				});

				row.appendChild(left);
				row.appendChild(delBtn);
				container.appendChild(row);
			});
		} catch (err) {
			console.error("クリップボードの取得失敗:", err);
		}
	}

	// ── 連絡先 ──
	const modalContact = document.getElementById("modal-contact");
	const contactForm = document.getElementById("contact-form");

	function openContactModal(contact) {
		document.getElementById("contact-id").value = contact ? contact.id : "";
		document.getElementById("contact-name").value = contact ? contact.name : "";
		document.getElementById("contact-birthday").value = contact
			? contact.birthday || ""
			: "";
		document.getElementById("contact-relationship").value = contact
			? contact.relationship || ""
			: "";
		document.getElementById("contact-info").value = contact
			? contact.contact_info || ""
			: "";
		document.getElementById("contact-tags").value = contact
			? (contact.tags || []).join(", ")
			: "";
		document.getElementById("contact-notes").value = contact
			? contact.notes || ""
			: "";
		document.getElementById("contact-modal-title").textContent = contact
			? `連絡先の編集: ${contact.name}`
			: "連絡先の追加";
		openModal(modalContact);
	}

	const btnNewContact = document.getElementById("btn-new-contact");
	if (btnNewContact) {
		btnNewContact.addEventListener("click", () => openContactModal(null));
	}

	if (contactForm) {
		contactForm.addEventListener("submit", async (e) => {
			e.preventDefault();
			const idVal = document.getElementById("contact-id").value;
			const tagsRaw = document.getElementById("contact-tags").value.trim();
			const payload = {
				...(idVal ? { id: Number(idVal) } : {}),
				name: document.getElementById("contact-name").value.trim(),
				birthday: document.getElementById("contact-birthday").value.trim(),
				relationship: document
					.getElementById("contact-relationship")
					.value.trim(),
				contactInfo: document.getElementById("contact-info").value.trim(),
				notes: document.getElementById("contact-notes").value.trim(),
				tags: tagsRaw
					? tagsRaw
							.split(",")
							.map((t) => t.trim())
							.filter((t) => t.length > 0)
					: [],
			};

			try {
				const res = await fetch("/api/contacts/save", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify(payload),
				});
				const data = await res.json();
				if (data.success) {
					closeModal(modalContact);
					contactForm.reset();
					fetchContactsList();
				} else {
					alert(data.message || "保存に失敗しました。");
				}
			} catch (err) {
				alert("通信エラーが発生しました。");
			}
		});
	}

	async function fetchContactsList() {
		const tbody = document.getElementById("contacts-tbody");
		if (!tbody) return;
		tbody.replaceChildren();

		try {
			const res = await fetch("/api/contacts");
			const data = await res.json();
			if (!data.success || !data.contacts || data.contacts.length === 0) {
				const tr = document.createElement("tr");
				const td = document.createElement("td");
				td.colSpan = 6;
				td.style.textAlign = "center";
				td.style.padding = "16px";
				td.style.color = "var(--color-zinc-muted)";
				td.style.fontSize = "0.8rem";
				td.textContent = "連絡先は登録されていません。";
				tr.appendChild(td);
				tbody.appendChild(tr);
				return;
			}

			data.contacts.forEach((contact) => {
				const tr = document.createElement("tr");

				const tdName = document.createElement("td");
				tdName.style.fontWeight = "600";
				tdName.textContent = contact.name;
				tr.appendChild(tdName);

				const tdBirthday = document.createElement("td");
				tdBirthday.style.fontFamily = "var(--font-family-mono)";
				tdBirthday.style.fontSize = "0.8rem";
				tdBirthday.textContent = contact.birthday || "—";
				tr.appendChild(tdBirthday);

				const tdRel = document.createElement("td");
				tdRel.textContent = contact.relationship || "—";
				tr.appendChild(tdRel);

				const tdInfo = document.createElement("td");
				tdInfo.style.fontSize = "0.8rem";
				tdInfo.textContent = contact.contact_info || "—";
				tr.appendChild(tdInfo);

				const tdTags = document.createElement("td");
				(contact.tags || []).forEach((tag) => {
					const chip = document.createElement("span");
					chip.className = "tag-chip";
					chip.textContent = `#${tag}`;
					tdTags.appendChild(chip);
				});
				if (!contact.tags || contact.tags.length === 0)
					tdTags.textContent = "—";
				tr.appendChild(tdTags);

				const tdActions = document.createElement("td");
				tdActions.style.textAlign = "right";

				const editBtn = document.createElement("button");
				editBtn.type = "button";
				editBtn.className = "btn btn-secondary btn-sm";
				editBtn.style.cssText =
					"font-size:0.72rem;padding:3px 8px;margin-right:4px;";
				editBtn.textContent = "編集";
				editBtn.addEventListener("click", () => openContactModal(contact));
				tdActions.appendChild(editBtn);

				const delBtn = document.createElement("button");
				delBtn.type = "button";
				delBtn.className = "btn btn-secondary btn-sm";
				delBtn.style.cssText = "font-size:0.72rem;padding:3px 8px;";
				delBtn.innerHTML = `<span class="material-symbols-outlined" style="font-size:13px;">delete</span>`;
				delBtn.addEventListener("click", async () => {
					if (!confirm(`連絡先「${contact.name}」を削除しますか？`)) return;
					try {
						const r = await fetch("/api/contacts/delete", {
							method: "POST",
							headers: { "Content-Type": "application/json" },
							body: JSON.stringify({ id: contact.id }),
						});
						const d = await r.json();
						if (d.success) fetchContactsList();
					} catch (err) {
						console.error(err);
					}
				});
				tdActions.appendChild(delBtn);
				tr.appendChild(tdActions);

				tbody.appendChild(tr);
			});
		} catch (err) {
			console.error("連絡先の取得失敗:", err);
		}
	}

	// ==========================================
	// DELIVERY: 朝報・日報・週報 (§3.8, §3.9)
	// ==========================================
	let briefingFeeds = [];

	function renderBriefingFeeds() {
		const list = document.getElementById("briefing-feeds-list");
		if (!list) return;
		list.replaceChildren();

		if (briefingFeeds.length === 0) {
			const empty = document.createElement("p");
			empty.style.cssText =
				"font-size:0.78rem;color:var(--text-secondary);margin:0;";
			empty.textContent = "フィードは登録されていません。";
			list.appendChild(empty);
			return;
		}

		briefingFeeds.forEach((feed, idx) => {
			const row = document.createElement("div");
			row.style.cssText =
				"display:flex;align-items:center;justify-content:space-between;gap:8px;padding:6px 10px;border:1px solid var(--border-matte);border-radius:var(--radius);";

			const urlSpan = document.createElement("span");
			urlSpan.style.cssText =
				"font-size:0.8rem;font-family:var(--font-family-mono);overflow:hidden;text-overflow:ellipsis;white-space:nowrap;flex:1;";
			urlSpan.title = feed;
			urlSpan.textContent = feed;

			const removeBtn = document.createElement("button");
			removeBtn.type = "button";
			removeBtn.className = "btn-trash";
			removeBtn.style.cssText = "width:26px;height:26px;flex-shrink:0;";
			removeBtn.innerHTML = `<span class="material-symbols-outlined" style="font-size:15px;">close</span>`;
			removeBtn.addEventListener("click", () => {
				briefingFeeds.splice(idx, 1);
				renderBriefingFeeds();
			});

			row.appendChild(urlSpan);
			row.appendChild(removeBtn);
			list.appendChild(row);
		});
	}

	const btnAddBriefingFeed = document.getElementById("btn-add-briefing-feed");
	if (btnAddBriefingFeed) {
		btnAddBriefingFeed.addEventListener("click", () => {
			const input = document.getElementById("briefing-feed-input");
			const url = input.value.trim();
			if (!url) return;
			if (briefingFeeds.includes(url)) {
				alert("同じフィードが既に登録されています。");
				return;
			}
			briefingFeeds.push(url);
			input.value = "";
			renderBriefingFeeds();
		});
	}

	async function fetchBriefingConfig() {
		try {
			const res = await fetch("/api/briefing-config");
			const data = await res.json();
			if (!data.success) return;

			const c = data.config;
			document.getElementById("briefing-enabled").checked = !!(c && c.enabled);
			document.getElementById("briefing-cron").value =
				c && c.schedule_cron ? c.schedule_cron : "";
			document.getElementById("briefing-target-type").value =
				c && c.target_type === "channel" ? "channel" : "dm";
			document.getElementById("briefing-target-id").value =
				(c && c.target_id) || "";
			document.getElementById("briefing-location-name").value =
				(c && c.location_name) || "";
			document.getElementById("briefing-weather-lat").value =
				c && c.weather_lat != null ? c.weather_lat : "";
			document.getElementById("briefing-weather-lng").value =
				c && c.weather_lng != null ? c.weather_lng : "";
			document.getElementById("briefing-keywords").value =
				c && Array.isArray(c.news_keywords) ? c.news_keywords.join(", ") : "";
			briefingFeeds =
				c && Array.isArray(c.news_feeds) ? c.news_feeds.slice() : [];
			renderBriefingFeeds();
		} catch (err) {
			console.error("朝報設定の取得失敗:", err);
		}
	}

	const briefingConfigForm = document.getElementById("briefing-config-form");
	if (briefingConfigForm) {
		briefingConfigForm.addEventListener("submit", async (e) => {
			e.preventDefault();
			const keywordsRaw = document
				.getElementById("briefing-keywords")
				.value.trim();
			const latVal = document.getElementById("briefing-weather-lat").value;
			const lngVal = document.getElementById("briefing-weather-lng").value;
			const cronVal = document.getElementById("briefing-cron").value.trim();

			const payload = {
				enabled: document.getElementById("briefing-enabled").checked,
				...(cronVal ? { schedule_cron: cronVal } : {}),
				target_type: document.getElementById("briefing-target-type").value,
				target_id: document.getElementById("briefing-target-id").value.trim(),
				weather_lat: latVal === "" ? null : Number(latVal),
				weather_lng: lngVal === "" ? null : Number(lngVal),
				location_name: document
					.getElementById("briefing-location-name")
					.value.trim(),
				news_feeds: briefingFeeds,
				news_keywords: keywordsRaw
					? keywordsRaw
							.split(",")
							.map((k) => k.trim())
							.filter((k) => k.length > 0)
					: [],
			};

			try {
				const res = await fetch("/api/briefing-config", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify(payload),
				});
				const data = await res.json();
				alert(
					data.message ||
						(data.success ? "保存しました。" : "保存に失敗しました。"),
				);
			} catch (err) {
				alert("通信エラーが発生しました。");
			}
		});
	}

	const btnBriefingTest = document.getElementById("btn-briefing-test");
	if (btnBriefingTest) {
		btnBriefingTest.addEventListener("click", async () => {
			btnBriefingTest.disabled = true;
			try {
				const res = await fetch("/api/briefing/test", { method: "POST" });
				const data = await res.json();
				alert(
					data.message ||
						(data.success ? "テスト配信しました。" : "配信に失敗しました。"),
				);
			} catch (err) {
				alert("通信エラーが発生しました。");
			} finally {
				btnBriefingTest.disabled = false;
			}
		});
	}

	async function fetchReportConfigs() {
		try {
			const res = await fetch("/api/report-configs");
			const data = await res.json();
			if (!data.success || !Array.isArray(data.configs)) return;

			data.configs.forEach((c) => {
				if (c.type !== "daily" && c.type !== "weekly") return;
				const enabledEl = document.getElementById(`report-${c.type}-enabled`);
				const cronEl = document.getElementById(`report-${c.type}-cron`);
				if (enabledEl) enabledEl.checked = !!c.enabled;
				if (cronEl) cronEl.value = c.schedule_cron || "";
			});
		} catch (err) {
			console.error("レポート設定の取得失敗:", err);
		}
	}

	function bindReportForm(type) {
		const form = document.getElementById(`report-${type}-form`);
		if (form) {
			form.addEventListener("submit", async (e) => {
				e.preventDefault();
				const cronVal = document
					.getElementById(`report-${type}-cron`)
					.value.trim();
				const payload = {
					type,
					enabled: document.getElementById(`report-${type}-enabled`).checked,
					...(cronVal ? { schedule_cron: cronVal } : {}),
				};
				try {
					const res = await fetch("/api/report-configs", {
						method: "POST",
						headers: { "Content-Type": "application/json" },
						body: JSON.stringify(payload),
					});
					const data = await res.json();
					alert(
						data.message ||
							(data.success ? "保存しました。" : "保存に失敗しました。"),
					);
				} catch (err) {
					alert("通信エラーが発生しました。");
				}
			});
		}

		const testBtn = document.getElementById(`btn-report-${type}-test`);
		if (testBtn) {
			testBtn.addEventListener("click", async () => {
				testBtn.disabled = true;
				try {
					const res = await fetch("/api/report-configs/test", {
						method: "POST",
						headers: { "Content-Type": "application/json" },
						body: JSON.stringify({ type }),
					});
					const data = await res.json();
					alert(
						data.message ||
							(data.success ? "テスト配信しました。" : "配信に失敗しました。"),
					);
				} catch (err) {
					alert("通信エラーが発生しました。");
				} finally {
					testBtn.disabled = false;
				}
			});
		}
	}
	bindReportForm("daily");
	bindReportForm("weekly");

	// ==========================================
	// WEBHOOKS MANAGEMENT (§3.13)
	// ==========================================
	const modalWebhook = document.getElementById("modal-webhook");
	const btnNewWebhook = document.getElementById("btn-new-webhook");
	if (btnNewWebhook && modalWebhook) {
		btnNewWebhook.addEventListener("click", () => openModal(modalWebhook));
	}

	const webhookForm = document.getElementById("webhook-form");
	if (webhookForm) {
		webhookForm.addEventListener("submit", async (e) => {
			e.preventDefault();
			const payload = {
				name: document.getElementById("webhook-name").value.trim(),
				notifyTargetType: document.getElementById("webhook-notify-type").value,
				notifyTargetId: document
					.getElementById("webhook-notify-id")
					.value.trim(),
				createTodo: document.getElementById("webhook-create-todo").checked,
				createReminder: document.getElementById("webhook-create-reminder")
					.checked,
			};
			const secret = document.getElementById("webhook-secret").value;
			const template = document.getElementById("webhook-template").value.trim();
			const filterKeyword = document
				.getElementById("webhook-filter")
				.value.trim();
			if (secret) payload.secret = secret;
			if (template) payload.template = template;
			if (filterKeyword) payload.filterKeyword = filterKeyword;

			try {
				const res = await fetch("/api/webhooks/create", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify(payload),
				});
				const data = await res.json();
				if (data.success) {
					closeModal(modalWebhook);
					webhookForm.reset();
					alert(data.message || "Webhookを作成しました。");
					fetchWebhooksList();
				} else {
					alert(data.message || "作成に失敗しました。");
				}
			} catch (err) {
				alert("通信エラーが発生しました。");
			}
		});
	}

	async function fetchWebhooksList() {
		const container = document.getElementById("webhooks-list");
		if (!container) return;
		container.replaceChildren();

		try {
			const res = await fetch("/api/webhooks");
			const data = await res.json();
			if (!data.success || !data.endpoints || data.endpoints.length === 0) {
				const empty = document.createElement("div");
				empty.className = "glass";
				empty.style.cssText =
					"padding:16px;text-align:center;color:var(--color-zinc-muted);font-size:0.85rem;";
				empty.textContent =
					"Webhookエンドポイントはありません。「＋ Webhook作成」から発行できます。";
				container.appendChild(empty);
				return;
			}

			data.endpoints.forEach((ep) => {
				const card = document.createElement("div");
				card.className = "card-item glass";
				card.style.cssText =
					"flex-direction:column;align-items:stretch;gap:8px;padding:14px 16px;";

				const topRow = document.createElement("div");
				topRow.style.cssText =
					"display:flex;justify-content:space-between;align-items:center;gap:8px;flex-wrap:wrap;";

				const titleWrap = document.createElement("div");
				titleWrap.style.cssText =
					"display:flex;align-items:center;gap:8px;flex-wrap:wrap;";

				const title = document.createElement("span");
				title.className = "card-title";
				title.style.fontSize = "0.95rem";
				title.textContent = ep.name;
				titleWrap.appendChild(title);

				const enabledBadge = document.createElement("span");
				enabledBadge.className = `status-badge ${ep.enabled ? "status-sent" : "status-cancelled"}`;
				enabledBadge.textContent = ep.enabled ? "有効" : "無効";
				titleWrap.appendChild(enabledBadge);

				if (ep.has_secret) {
					const secretBadge = document.createElement("span");
					secretBadge.className = "status-badge status-pending";
					secretBadge.textContent = "署名検証";
					titleWrap.appendChild(secretBadge);
				}

				const actions = document.createElement("div");
				actions.style.cssText = "display:flex;gap:6px;flex-wrap:wrap;";

				const toggleBtn = document.createElement("button");
				toggleBtn.type = "button";
				toggleBtn.className = `btn ${ep.enabled ? "btn-secondary" : "btn-primary"} btn-sm`;
				toggleBtn.style.cssText = "font-size:0.72rem;padding:3px 10px;";
				toggleBtn.textContent = ep.enabled ? "無効化" : "有効化";
				toggleBtn.addEventListener("click", async () => {
					try {
						const r = await fetch("/api/webhooks/update", {
							method: "POST",
							headers: { "Content-Type": "application/json" },
							body: JSON.stringify({ id: ep.id, enabled: !ep.enabled }),
						});
						const d = await r.json();
						if (d.success) fetchWebhooksList();
						else alert(d.message || "更新に失敗しました。");
					} catch (err) {
						alert("通信エラーが発生しました。");
					}
				});
				actions.appendChild(toggleBtn);

				const delBtn = document.createElement("button");
				delBtn.type = "button";
				delBtn.className = "btn btn-secondary btn-sm";
				delBtn.style.cssText = "font-size:0.72rem;padding:3px 10px;";
				delBtn.textContent = "削除";
				delBtn.addEventListener("click", async () => {
					if (
						!confirm(
							`Webhook「${ep.name}」を削除しますか？\n発行済みURLは無効になります。`,
						)
					)
						return;
					try {
						const r = await fetch("/api/webhooks/delete", {
							method: "POST",
							headers: { "Content-Type": "application/json" },
							body: JSON.stringify({ id: ep.id }),
						});
						const d = await r.json();
						if (d.success) {
							fetchWebhooksList();
							fetchWebhookDeliveries();
						} else {
							alert(d.message || "削除に失敗しました。");
						}
					} catch (err) {
						alert("通信エラーが発生しました。");
					}
				});
				actions.appendChild(delBtn);

				topRow.appendChild(titleWrap);
				topRow.appendChild(actions);

				// URL コピー行
				const urlRow = document.createElement("div");
				urlRow.style.cssText = "display:flex;align-items:center;gap:8px;";

				const urlInput = document.createElement("input");
				urlInput.type = "text";
				urlInput.readOnly = true;
				urlInput.value = ep.url;
				urlInput.style.cssText =
					"flex:1;font-family:var(--font-family-mono);font-size:0.78rem;";
				urlInput.addEventListener("focus", () => urlInput.select());

				const copyBtn = document.createElement("button");
				copyBtn.type = "button";
				copyBtn.className = "btn btn-secondary btn-sm";
				copyBtn.style.cssText =
					"font-size:0.72rem;padding:3px 10px;white-space:nowrap;";
				copyBtn.textContent = "URLコピー";
				copyBtn.addEventListener("click", async () => {
					try {
						await navigator.clipboard.writeText(ep.url);
						copyBtn.textContent = "コピーしました";
						setTimeout(() => {
							copyBtn.textContent = "URLコピー";
						}, 1500);
					} catch (err) {
						urlInput.select();
						alert("コピーに失敗しました。手動で選択してコピーしてください。");
					}
				});

				urlRow.appendChild(urlInput);
				urlRow.appendChild(copyBtn);

				// メタ情報
				const meta = document.createElement("div");
				meta.style.cssText =
					"font-size:0.75rem;color:var(--color-zinc-muted);display:flex;gap:14px;flex-wrap:wrap;";
				const metaItems = [];
				metaItems.push(
					ep.notify_target_type === "channel"
						? `通知先: チャンネル${ep.notify_target_id ? ` (${ep.notify_target_id})` : ""}`
						: "通知先: DM",
				);
				if (ep.filter_keyword) metaItems.push(`フィルタ: ${ep.filter_keyword}`);
				if (ep.create_todo) metaItems.push("ToDo自動作成");
				if (ep.create_reminder) metaItems.push("リマインダー自動作成");
				metaItems.forEach((item) => {
					const span = document.createElement("span");
					span.textContent = item;
					meta.appendChild(span);
				});

				card.appendChild(topRow);
				card.appendChild(urlRow);
				card.appendChild(meta);
				container.appendChild(card);
			});
		} catch (err) {
			console.error("Webhook一覧の取得失敗:", err);
		}
	}

	async function fetchWebhookDeliveries() {
		const tbody = document.getElementById("webhook-deliveries-tbody");
		if (!tbody) return;
		tbody.replaceChildren();

		try {
			const res = await fetch("/api/webhooks/deliveries");
			const data = await res.json();
			if (!data.success || !data.deliveries || data.deliveries.length === 0) {
				const tr = document.createElement("tr");
				const td = document.createElement("td");
				td.colSpan = 3;
				td.style.textAlign = "center";
				td.style.padding = "16px";
				td.style.color = "var(--color-zinc-muted)";
				td.style.fontSize = "0.8rem";
				td.textContent = "受信履歴はありません。";
				tr.appendChild(td);
				tbody.appendChild(tr);
				return;
			}

			const statusLabels = {
				received: "受信",
				notified: "通知済み",
				filtered: "フィルタ済み",
				failed: "失敗",
			};

			data.deliveries.forEach((del) => {
				const tr = document.createElement("tr");

				const tdDate = document.createElement("td");
				tdDate.style.whiteSpace = "nowrap";
				tdDate.style.fontSize = "0.8rem";
				tdDate.textContent = del.created_at;
				tr.appendChild(tdDate);

				const tdStatus = document.createElement("td");
				const badge = document.createElement("span");
				badge.className = `status-badge status-delivery-${del.status}`;
				badge.textContent = statusLabels[del.status] || del.status;
				tdStatus.appendChild(badge);
				tr.appendChild(tdStatus);

				const tdDetail = document.createElement("td");
				tdDetail.style.fontSize = "0.78rem";
				tdDetail.style.color = "var(--text-secondary)";
				tdDetail.textContent = del.detail || "—";
				tr.appendChild(tdDetail);

				tbody.appendChild(tr);
			});
		} catch (err) {
			console.error("受信履歴の取得失敗:", err);
		}
	}

	const btnRefreshDeliveries = document.getElementById(
		"btn-refresh-deliveries",
	);
	if (btnRefreshDeliveries) {
		btnRefreshDeliveries.addEventListener("click", fetchWebhookDeliveries);
	}

	// ==========================================
	// MCP SERVERS MANAGEMENT (§4.4)
	// ==========================================

	// ダッシュボード（サードパーティMCPサーバー由来のSPA）は、サンドボックス iframe に隔離して埋め込む。
	// sandbox="allow-scripts" のみ（allow-same-origin は付けない）＝iframe は「不透明オリジン」となり、
	// ダッシュボードの JS は yuuka 本体の Cookie・localStorage・DOM・同一オリジンAPIへ一切触れない。
	// - HTML はサーバー側ルート（/api/mcp-servers/:id/dashboard）が text/html＋専用CSP で直接返すので、
	//   ここでは iframe.src に指定するだけ（DOMParser での再パースや script 再生成は不要）。
	// - /proxy への通信はトークン認証＋CORS(ACAO:null)で成立する（Cookie 不要）。
	// - CSS は iframe ドキュメント内に完全に閉じ込められるため、本体スタイルへの波及は起きない。

	// 埋め込んだ iframe を破棄する。
	function teardownMcpDashboard(container) {
		if (container) container.replaceChildren();
	}

	async function openMcpDashboard(server) {
		const modal = document.getElementById("modal-mcp-dashboard");
		const container = document.getElementById("mcp-dashboard-container");
		const titleEl = document.getElementById("mcp-dashboard-title");
		if (!modal || !container) return;
		if (titleEl) titleEl.textContent = `${server.name} の管理ページ`;
		teardownMcpDashboard(container);

		const iframe = document.createElement("iframe");
		// allow-scripts のみ。allow-same-origin は決して付けない（付けると本体オリジンと同一視され隔離が無効化される）。
		// allow-forms は SPA のフォーム送信のため。いずれも iframe を不透明オリジンに保ったまま機能する。
		iframe.setAttribute("sandbox", "allow-scripts allow-forms");
		iframe.style.cssText =
			"width:100%;height:75vh;border:0;display:block;background:#fff;";
		iframe.src = `/api/mcp-servers/${server.id}/dashboard`;
		container.appendChild(iframe);
		openModal(modal);
	}

	async function fetchMcpServersList() {
		// 現在操作対象のBot名をタブ先頭に表示する
		const botLabel = document.getElementById("mcp-current-bot-label");
		if (botLabel) {
			const currentBotId = window.currentBotId || "system_default";
			const currentBotName = localStorage.getItem("currentBotName");
			let displayName;
			if (currentBotId === "system_default" || !currentBotName) {
				displayName = "既定の秘書（早瀬ユウカ）";
			} else {
				displayName = currentBotName;
			}
			botLabel.textContent = `現在のBot: ${displayName}`;
		}

		// Adminの場合のみスコープ選択を表示
		const scopeGroup = document.getElementById("mcp-scope-group");
		if (scopeGroup) {
			scopeGroup.style.display = activeUserRole === "admin" ? "" : "none";
		}

		const container = document.getElementById("mcp-servers-list");
		if (!container) return;
		container.replaceChildren();

		try {
			const res = await fetch("/api/mcp-servers");
			const data = await res.json();
			if (!data.success || !data.servers || data.servers.length === 0) {
				const empty = document.createElement("div");
				empty.className = "glass";
				empty.style.cssText =
					"padding:16px;text-align:center;color:var(--color-zinc-muted);font-size:0.85rem;";
				empty.textContent = "MCPサーバーは登録されていません。";
				container.appendChild(empty);
				return;
			}

			data.servers.forEach((server) => {
				const card = document.createElement("div");
				card.className = "card-item glass";
				card.style.cssText =
					"flex-direction:column;align-items:stretch;gap:8px;padding:14px 16px;";

				const topRow = document.createElement("div");
				topRow.style.cssText =
					"display:flex;justify-content:space-between;align-items:center;gap:8px;flex-wrap:wrap;";

				const titleWrap = document.createElement("div");
				titleWrap.style.cssText =
					"display:flex;align-items:center;gap:8px;flex-wrap:wrap;";

				const title = document.createElement("span");
				title.className = "card-title";
				title.style.fontSize = "0.95rem";
				title.textContent = server.name;
				titleWrap.appendChild(title);

				const scopeBadge = document.createElement("span");
				scopeBadge.className = "badge badge-accent";
				scopeBadge.style.fontSize = "0.65rem";
				scopeBadge.textContent =
					server.scope === "system" ? "システム" : "ユーザー";
				titleWrap.appendChild(scopeBadge);

				const enabledBadge = document.createElement("span");
				enabledBadge.className = `status-badge ${server.enabled ? "status-sent" : "status-cancelled"}`;
				enabledBadge.textContent = server.enabled ? "有効" : "無効";
				titleWrap.appendChild(enabledBadge);

				if (server.requires_confirmation) {
					const confirmBadge = document.createElement("span");
					confirmBadge.className = "status-badge status-pending";
					confirmBadge.textContent = "実行前確認";
					titleWrap.appendChild(confirmBadge);
				}

				topRow.appendChild(titleWrap);

				const url = document.createElement("div");
				url.style.cssText =
					"font-size:0.78rem;font-family:var(--font-family-mono);color:var(--color-zinc-muted);overflow:hidden;text-overflow:ellipsis;white-space:nowrap;";
				url.title = server.endpoint_url;
				url.textContent = `${server.endpoint_url}${server.has_auth ? " 🔑" : ""}`;

				// tools チップ一覧
				const toolsRow = document.createElement("div");
				toolsRow.style.cssText = "display:flex;flex-wrap:wrap;gap:6px;";
				if (server.tools && server.tools.length > 0) {
					server.tools.forEach((tool) => {
						const chip = document.createElement("span");
						chip.className = "tag-chip";
						chip.title = tool.description || "";
						chip.textContent = tool.name;
						toolsRow.appendChild(chip);
					});
				} else {
					const noTools = document.createElement("span");
					noTools.style.cssText =
						"font-size:0.75rem;color:var(--color-zinc-muted);";
					noTools.textContent = "Tool未取得（「再取得」をお試しください）";
					toolsRow.appendChild(noTools);
				}

				const actions = document.createElement("div");
				actions.style.cssText =
					"display:flex;gap:6px;flex-wrap:wrap;margin-top:4px;";

				const refreshBtn = document.createElement("button");
				refreshBtn.type = "button";
				refreshBtn.className = "btn btn-secondary btn-sm";
				refreshBtn.style.cssText = "font-size:0.72rem;padding:3px 10px;";
				refreshBtn.textContent = "Tool再取得";
				refreshBtn.addEventListener("click", async () => {
					refreshBtn.disabled = true;
					try {
						const r = await fetch("/api/mcp-servers/refresh", {
							method: "POST",
							headers: { "Content-Type": "application/json" },
							body: JSON.stringify({ id: server.id }),
						});
						const d = await r.json();
						alert(
							d.message ||
								(d.success ? "更新しました。" : "更新に失敗しました。"),
						);
						if (d.success) fetchMcpServersList();
					} catch (err) {
						alert("通信エラーが発生しました。");
					} finally {
						refreshBtn.disabled = false;
					}
				});
				actions.appendChild(refreshBtn);

				const toggleBtn = document.createElement("button");
				toggleBtn.type = "button";
				toggleBtn.className = `btn ${server.enabled ? "btn-secondary" : "btn-primary"} btn-sm`;
				toggleBtn.style.cssText = "font-size:0.72rem;padding:3px 10px;";
				toggleBtn.textContent = server.enabled ? "無効化" : "有効化";
				toggleBtn.addEventListener("click", async () => {
					try {
						const r = await fetch("/api/mcp-servers/toggle", {
							method: "POST",
							headers: { "Content-Type": "application/json" },
							body: JSON.stringify({ id: server.id, enabled: !server.enabled }),
						});
						const d = await r.json();
						if (d.success) fetchMcpServersList();
						else alert(d.message || "操作に失敗しました。");
					} catch (err) {
						alert("通信エラーが発生しました。");
					}
				});
				actions.appendChild(toggleBtn);

				const delBtn = document.createElement("button");
				delBtn.type = "button";
				delBtn.className = "btn btn-secondary btn-sm";
				delBtn.style.cssText = "font-size:0.72rem;padding:3px 10px;";
				delBtn.textContent = "削除";
				delBtn.addEventListener("click", async () => {
					if (!confirm(`MCPサーバー「${server.name}」を削除しますか？`)) return;
					try {
						const r = await fetch("/api/mcp-servers/delete", {
							method: "POST",
							headers: { "Content-Type": "application/json" },
							body: JSON.stringify({ id: server.id }),
						});
						const d = await r.json();
						if (d.success) fetchMcpServersList();
						else alert(d.message || "削除に失敗しました。");
					} catch (err) {
						alert("通信エラーが発生しました。");
					}
				});
				actions.appendChild(delBtn);

				// 管理ページ提供がある場合のみ「管理ページ」ボタンを表示（fire-and-forget で判定）。
				// <origin>/dashboard/enable が200を返すサーバーのみ available:true。
				fetch(`/api/mcp-servers/${server.id}/dashboard/status`)
					.then((r) => r.json())
					.then((d) => {
						if (!d || !d.available) return;
						const dashBtn = document.createElement("button");
						dashBtn.type = "button";
						dashBtn.className = "btn btn-secondary btn-sm";
						dashBtn.style.cssText = "font-size:0.72rem;padding:3px 10px;";
						dashBtn.textContent = "管理ページ";
						dashBtn.addEventListener("click", () => openMcpDashboard(server));
						// 削除ボタンの前に差し込む（管理系操作をまとめる）
						actions.insertBefore(dashBtn, delBtn);
					})
					.catch(() => {});

				card.appendChild(topRow);
				card.appendChild(url);
				card.appendChild(toolsRow);
				card.appendChild(actions);
				container.appendChild(card);
			});
		} catch (err) {
			console.error("MCPサーバー一覧の取得失敗:", err);
		}
	}

	const mcpAddForm = document.getElementById("mcp-add-form");
	if (mcpAddForm) {
		mcpAddForm.addEventListener("submit", async (e) => {
			e.preventDefault();
			const submitBtn = mcpAddForm.querySelector("button[type=submit]");
			const payload = {
				name: document.getElementById("mcp-name").value.trim(),
				endpointUrl: document.getElementById("mcp-endpoint-url").value.trim(),
				requiresConfirmation: document.getElementById(
					"mcp-requires-confirmation",
				).checked,
			};
			const authCredential = document.getElementById(
				"mcp-auth-credential",
			).value;
			if (authCredential) payload.authCredential = authCredential;
			if (activeUserRole === "admin") {
				payload.scope =
					document.getElementById("mcp-scope").value === "system"
						? "system"
						: "user";
			}

			if (submitBtn) submitBtn.disabled = true;
			try {
				const res = await fetch("/api/mcp-servers/add", {
					method: "POST",
					headers: { "Content-Type": "application/json" },
					body: JSON.stringify(payload),
				});
				const data = await res.json();
				alert(
					data.message ||
						(data.success ? "登録しました。" : "登録に失敗しました。"),
				);
				if (data.success) {
					mcpAddForm.reset();
					document.getElementById("mcp-requires-confirmation").checked = true;
					fetchMcpServersList();
				}
			} catch (err) {
				alert("通信エラーが発生しました。");
			} finally {
				if (submitBtn) submitBtn.disabled = false;
			}
		});
	}

	// ==========================================
	// BOT統合管理（owner単位の横断ページ, v5）
	// ==========================================
	let intOverview = null; // 直近のオーバービュー
	let intWired = false; // フォーム等の一度きり配線フラグ

	const intEsc = (s) =>
		String(s == null ? "" : s).replace(
			/[&<>"']/g,
			(c) =>
				({
					"&": "&amp;",
					"<": "&lt;",
					">": "&gt;",
					'"': "&quot;",
					"'": "&#39;",
				})[c],
		);

	async function intPost(url, body) {
		const res = await fetch(url, {
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify(body || {}),
		});
		return res.json();
	}

	// Bot名解決（オーバービューから）
	function intBotName(botId) {
		const b = (intOverview?.bots || []).find((x) => x.id === botId);
		if (!b) return botId;
		return b.is_system_default ? "既定の秘書（早瀬ユウカ）" : b.name;
	}

	// リソース許可UI（チェックボックス羅列ではなく「リストから選択」方式）。
	// 許可済みBotは除去可能なチップで表示し、未許可Botは <select> から選んで追加する。
	// grantedSet: 許可済みbotIdのSet / onToggleFnName: window上のトグル関数名 / itemKey: リソース識別子
	function intGrantChips(grantedSet, onToggleFnName, itemKey) {
		const bots = intOverview?.bots || [];
		const fnAttr = intEsc(onToggleFnName);
		const keyAttr = intEsc(JSON.stringify(itemKey));
		const granted = bots.filter((b) => grantedSet.has(b.id));
		const available = bots.filter((b) => !grantedSet.has(b.id));

		const chips = granted.length
			? granted
					.map(
						(
							b,
						) => `<span style="display:inline-flex;align-items:center;gap:6px;font-size:0.78rem;background:var(--surface-1dp,rgba(255,255,255,0.04));border:1px solid var(--border-matte,#333);border-radius:6px;padding:3px 6px 3px 10px;">
          ${intEsc(intBotName(b.id))}
          <button type="button" title="許可を解除" aria-label="${intEsc(intBotName(b.id))} の許可を解除" style="background:none;border:0;color:var(--text-secondary,#a1a1aa);cursor:pointer;font-size:1rem;line-height:1;padding:0 2px;" data-int-grant-fn="${fnAttr}" data-int-grant-bot="${intEsc(b.id)}" data-int-grant-key="${keyAttr}" data-int-grant-action="revoke">×</button>
        </span>`,
					)
					.join("")
			: `<span style="font-size:0.75rem;color:var(--text-secondary,#71717a);">許可中のBotはありません</span>`;

		const select = available.length
			? `<select data-int-grant-fn="${fnAttr}" data-int-grant-key="${keyAttr}" data-int-grant-action="grant" title="Botを追加" style="font-size:1rem;line-height:1;width:auto;padding:2px 6px;border:0;background:transparent;color:var(--text-secondary,#a1a1aa);cursor:pointer;-webkit-appearance:none;appearance:none;">
          <option value="" style="color:var(--text-high,#fff);background:var(--surface-2dp,#27272a);">＋</option>
          ${available.map((b) => `<option value="${intEsc(b.id)}" style="color:var(--text-high,#fff);background:var(--surface-2dp,#27272a);">${intEsc(intBotName(b.id))}</option>`).join("")}
        </select>`
			: "";

		return `${chips}${select}`;
	}

	// 許可UI（追加=select の change / 解除=× ボタンの click）を委譲処理（CSP対応: inline ハンドラを排除）。
	// トグル関数はローカル状態を更新するため、成功後に該当リストを再描画して即時反映する。
	function wireIntGrantChips(containerEl) {
		if (!containerEl) return;
		const parseKey = (el) => {
			try {
				return JSON.parse(el.getAttribute("data-int-grant-key"));
			} catch (e) {
				return el.getAttribute("data-int-grant-key");
			}
		};
		const apply = async (fnName, botId, key, granted) => {
			const fn = window[fnName];
			if (typeof fn !== "function") return;
			await fn(botId, key, granted);
			// ローカル状態(intOverview)から該当リストのみ再描画（ネットワーク不要）。
			if (fnName === "intToggleCred")
				renderIntCredentials(
					intOverview?.credentials || [],
					intOverview?.bots || [],
				);
			else if (fnName === "intToggleMcp")
				renderIntMcp(intOverview?.mcpServers || [], intOverview?.bots || []);
		};
		containerEl
			.querySelectorAll("[data-int-grant-action='grant']")
			.forEach((sel) => {
				sel.addEventListener("change", () => {
					const botId = sel.value;
					if (!botId) return;
					apply(
						sel.getAttribute("data-int-grant-fn"),
						botId,
						parseKey(sel),
						true,
					);
				});
			});
		containerEl
			.querySelectorAll("[data-int-grant-action='revoke']")
			.forEach((btn) => {
				btn.addEventListener("click", () => {
					apply(
						btn.getAttribute("data-int-grant-fn"),
						btn.getAttribute("data-int-grant-bot"),
						parseKey(btn),
						false,
					);
				});
			});
	}

	async function fetchIntegratedOverview() {
		try {
			const res = await fetch("/api/integrated/overview");
			const data = await res.json();
			if (!data.success) return;
			intOverview = data;
			// Admin はMCPのシステムスコープ選択を表示
			const scopeGroup = document.getElementById("int-mcp-scope-group");
			if (scopeGroup)
				scopeGroup.style.display = activeUserRole === "admin" ? "" : "none";
			renderIntBots(data.bots);
			renderIntCredentials(data.credentials, data.bots);
			renderIntMcp(data.mcpServers, data.bots);
			renderIntGoogle(data.googleAccounts, data.bots);
			if (!intWired) wireIntegratedForms();
		} catch (err) {
			console.error("統合管理オーバービューの取得に失敗:", err);
		}
	}

	function renderIntBots(bots) {
		const el = document.getElementById("int-bots-list");
		if (!el) return;
		el.innerHTML = "";
		bots.forEach((b) => {
			let chip, chipColor;
			if (b.suspended) {
				chip = "停止中(管理者)";
				chipColor = "#ef4444";
			} else if (b.connected) {
				chip = "稼働中";
				chipColor = "#10b981";
			} else if (b.running) {
				chip = "接続中…";
				chipColor = "#f59e0b";
			} else if (!b.has_token && !b.is_system_default) {
				chip = "トークン未設定";
				chipColor = "#71717a";
			} else {
				chip = "停止";
				chipColor = "#71717a";
			}
			const row = document.createElement("div");
			row.className = "glass";
			row.style.cssText =
				"display:flex;align-items:center;justify-content:space-between;gap:12px;padding:12px 14px;border-radius:8px;";
			const canControl = !b.is_system_default;

			// 主ボタン: 状態に応じて 起動/停止 を1つだけ大きく出す（system_default は制御不可）。
			let primaryBtn = "";
			if (canControl) {
				if (b.suspended || !b.has_token) {
					primaryBtn = `<button class="btn btn-secondary btn-sm" disabled>${b.running ? "停止" : "起動"}</button>`;
				} else if (b.running) {
					primaryBtn = `<button class="btn btn-secondary btn-sm" data-int-do="stop" data-int-bot="${b.id}">停止</button>`;
				} else {
					primaryBtn = `<button class="btn btn-secondary btn-sm" data-int-do="start" data-int-bot="${b.id}">起動</button>`;
				}
			}

			// ⋮ メニュー: 再起動（制御可Botのみ）＋ 会話履歴クリア（全Bot＝自分の会話に作用）。
			const menuItems = [];
			if (canControl) {
				const rd = b.suspended || !b.has_token ? "disabled" : "";
				menuItems.push(
					`<button class="int-menu-item" data-int-do="restart" data-int-bot="${b.id}" ${rd}>再起動</button>`,
				);
			}
			menuItems.push(
				`<button class="int-menu-item int-menu-danger" data-int-do="clear" data-int-bot="${b.id}" title="このBotとの自分の会話履歴をクリア（Redisキャッシュ削除＋境界記録。永続ログは保持）">会話履歴をクリア</button>`,
			);

			row.innerHTML = `
        <div style="display:flex;align-items:center;gap:10px;min-width:0;">
          <span class="material-symbols-outlined" style="color:${chipColor};">${b.is_system_default ? "shield_person" : "smart_toy"}</span>
          <div style="min-width:0;">
            <div style="font-weight:600;">${intEsc(b.name)} ${b.is_system_default ? '<span style="font-size:0.7rem;color:var(--color-zinc-muted,#a1a1aa);">(共有秘書)</span>' : ""}</div>
            <div style="font-size:0.75rem;color:var(--color-zinc-muted,#a1a1aa);">${intEsc(b.preset)}・${intEsc(b.discord_username || b.id)}</div>
          </div>
        </div>
        <div style="display:flex;align-items:center;gap:8px;flex-shrink:0;position:relative;">
          <span style="font-size:0.75rem;color:${chipColor};font-weight:600;">${chip}</span>
          ${primaryBtn}
          <button class="int-menu-toggle" data-int-menu="${b.id}" title="その他の操作">⋮</button>
          <div class="int-action-menu" data-int-menu-for="${b.id}">${menuItems.join("")}</div>
        </div>`;
			el.appendChild(row);
		});
		// 操作ボタン（主ボタン＋メニュー内）。clear だけ確認付きハンドラ、他は起動/停止/再起動。
		el.querySelectorAll("[data-int-do]").forEach((btn) =>
			btn.addEventListener("click", (e) => {
				e.stopPropagation();
				const action = btn.getAttribute("data-int-do");
				const botId = btn.getAttribute("data-int-bot");
				closeIntMenus();
				if (action === "clear") intClearHistory(botId);
				else intBotAction(action, botId);
			}),
		);
		// ⋮ トグル（他のメニューは閉じる）
		el.querySelectorAll("[data-int-menu]").forEach((btn) =>
			btn.addEventListener("click", (e) => {
				e.stopPropagation();
				const botId = btn.getAttribute("data-int-menu");
				const menu = el.querySelector(
					`.int-action-menu[data-int-menu-for="${botId}"]`,
				);
				const willOpen = menu && !menu.classList.contains("open");
				closeIntMenus();
				if (willOpen) menu.classList.add("open");
			}),
		);
	}

	function closeIntMenus() {
		document
			.querySelectorAll(".int-action-menu.open")
			.forEach((m) => m.classList.remove("open"));
	}

	async function intClearHistory(botId) {
		if (
			!confirm(
				"このBotとの会話履歴をクリアしますか？\n次のメッセージから新しい会話になります（永続ログは保持され、検索・監査では引き続き利用できます）。",
			)
		)
			return;
		const d = await intPost("/api/integrated/bots/clear-history", { botId });
		alert(
			d.message || (d.success ? "クリアしました。" : "クリアに失敗しました。"),
		);
	}

	async function intBotAction(action, botId) {
		const d = await intPost(`/api/integrated/bots/${action}`, { botId });
		if (!d.success && d.message) alert(d.message);
		// stop は即時反映、start/restart は Discord 接続確立を待って少し遅らせて再取得する
		setTimeout(fetchIntegratedOverview, action === "stop" ? 300 : 1500);
	}

	// ── 認証情報 ──
	function renderIntCredentials(creds, bots) {
		const el = document.getElementById("int-cred-list");
		if (!el) return;
		el.innerHTML = "";
		if (!creds.length) {
			el.innerHTML = '<p class="description-text">未登録です。</p>';
			return;
		}
		creds.forEach((c) => {
			const grantedSet = new Set(
				bots
					.filter((b) => (b.granted_credentials || []).includes(c.service_name))
					.map((b) => b.id),
			);
			const card = document.createElement("div");
			card.className = "glass";
			card.style.cssText = "padding:12px 14px;border-radius:8px;";
			card.innerHTML = `
        <div style="display:flex;justify-content:space-between;align-items:center;gap:8px;">
          <div><strong>${intEsc(c.service_name)}</strong> <span style="font-size:0.78rem;color:var(--color-zinc-muted,#a1a1aa);">${intEsc(c.username)}</span></div>
          <button class="btn btn-secondary btn-sm" data-int-cred-del="${intEsc(c.service_name)}">削除</button>
        </div>
        <div style="display:flex;flex-wrap:wrap;align-items:center;gap:6px;margin-top:8px;">${intGrantChips(grantedSet, "intToggleCred", c.service_name)}</div>`;
			el.appendChild(card);
		});
		wireIntGrantChips(el);
		el.querySelectorAll("[data-int-cred-del]").forEach((btn) =>
			btn.addEventListener("click", async () => {
				const svc = btn.getAttribute("data-int-cred-del");
				if (!confirm(`認証情報「${svc}」を削除しますか？`)) return;
				const d = await intPost("/api/credentials/delete", {
					serviceName: svc,
				});
				if (!d.success && d.message) alert(d.message);
				fetchIntegratedOverview();
			}),
		);
	}

	window.intToggleCred = async (botId, serviceName, granted) => {
		const d = await intPost("/api/integrated/grants/credential", {
			botId,
			serviceName,
			granted,
		});
		if (!d.success) {
			alert(d.message || "更新に失敗しました。");
			fetchIntegratedOverview();
		} else {
			const b = intOverview.bots.find((x) => x.id === botId);
			if (b) {
				b.granted_credentials = b.granted_credentials || [];
				if (granted) b.granted_credentials.push(serviceName);
				else
					b.granted_credentials = b.granted_credentials.filter(
						(s) => s !== serviceName,
					);
			}
		}
	};

	// ── MCP ──
	function renderIntMcp(servers, bots) {
		const el = document.getElementById("int-mcp-list");
		if (!el) return;
		el.innerHTML = "";
		if (!servers.length) {
			el.innerHTML = '<p class="description-text">未登録です。</p>';
			return;
		}
		servers.forEach((s) => {
			const grantedSet = new Set(
				bots
					.filter((b) => (b.granted_mcp_ids || []).includes(s.id))
					.map((b) => b.id),
			);
			const card = document.createElement("div");
			card.className = "glass";
			card.style.cssText = "padding:12px 14px;border-radius:8px;";
			card.innerHTML = `
        <div style="display:flex;justify-content:space-between;align-items:center;gap:8px;">
          <div style="min-width:0;"><strong>${intEsc(s.name)}</strong> <span style="font-size:0.72rem;color:${s.enabled ? "#10b981" : "#71717a"};">${s.enabled ? "有効" : "無効"}</span>
            <div style="font-size:0.72rem;color:var(--color-zinc-muted,#a1a1aa);word-break:break-all;">${intEsc(s.endpoint_url)} ・ ${s.tools} tools</div></div>
          <div style="display:flex;gap:6px;flex-shrink:0;" data-int-mcp-actions>
            <button class="btn btn-secondary btn-sm" data-int-mcp-toggle="${s.id}" data-enabled="${s.enabled ? 1 : 0}">${s.enabled ? "無効化" : "有効化"}</button>
            <button class="btn btn-secondary btn-sm" data-int-mcp-del="${s.id}">削除</button>
          </div>
        </div>
        <div style="display:flex;flex-wrap:wrap;align-items:center;gap:6px;margin-top:8px;">${intGrantChips(grantedSet, "intToggleMcp", s.id)}</div>`;
			el.appendChild(card);

			// MCP管理ページ（dashboard提供サーバーのみ）。fire-and-forgetで判定し、
			// 「無効化／削除」の前に「管理ページ」ボタンを差し込む。openMcpDashboard はモーダルで開く。
			const actionsBar = card.querySelector("[data-int-mcp-actions]");
			fetch(`/api/mcp-servers/${s.id}/dashboard/status`)
				.then((r) => r.json())
				.then((d) => {
					if (!d || !d.available || !actionsBar) return;
					const dashBtn = document.createElement("button");
					dashBtn.type = "button";
					dashBtn.className = "btn btn-secondary btn-sm";
					dashBtn.textContent = "管理ページ";
					dashBtn.addEventListener("click", () => openMcpDashboard(s));
					actionsBar.insertBefore(dashBtn, actionsBar.firstChild);
				})
				.catch(() => {});
		});
		wireIntGrantChips(el);
		el.querySelectorAll("[data-int-mcp-toggle]").forEach((btn) =>
			btn.addEventListener("click", async () => {
				const id = Number(btn.getAttribute("data-int-mcp-toggle"));
				const enabled = btn.getAttribute("data-enabled") === "1";
				await intPost("/api/mcp-servers/toggle", { id, enabled: !enabled });
				fetchIntegratedOverview();
			}),
		);
		el.querySelectorAll("[data-int-mcp-del]").forEach((btn) =>
			btn.addEventListener("click", async () => {
				const id = Number(btn.getAttribute("data-int-mcp-del"));
				if (!confirm("このMCPサーバーを削除しますか？")) return;
				await intPost("/api/mcp-servers/delete", { id });
				fetchIntegratedOverview();
			}),
		);
	}

	window.intToggleMcp = async (botId, serverId, granted) => {
		const d = await intPost("/api/integrated/grants/mcp", {
			botId,
			serverId,
			granted,
		});
		if (!d.success) {
			alert(d.message || "更新に失敗しました。");
			fetchIntegratedOverview();
		} else {
			const b = intOverview.bots.find((x) => x.id === botId);
			if (b) {
				b.granted_mcp_ids = b.granted_mcp_ids || [];
				if (granted) b.granted_mcp_ids.push(serverId);
				else
					b.granted_mcp_ids = b.granted_mcp_ids.filter((i) => i !== serverId);
			}
		}
	};

	// ── Google ──
	function renderIntGoogle(accounts, bots) {
		const listEl = document.getElementById("int-google-list");
		const assignEl = document.getElementById("int-google-bot-assign");
		if (listEl) {
			listEl.innerHTML = accounts.length
				? ""
				: '<p class="description-text">連携アカウントはありません。</p>';
			accounts.forEach((a) => {
				const card = document.createElement("div");
				card.className = "glass";
				card.style.cssText =
					"display:flex;justify-content:space-between;align-items:center;gap:8px;padding:12px 14px;border-radius:8px;";
				card.innerHTML = `
          <div><strong>${intEsc(a.email || "(メール不明)")}</strong> ${a.is_primary ? '<span style="font-size:0.7rem;color:#10b981;">primary</span>' : ""}
            <div style="font-size:0.72rem;color:var(--color-zinc-muted,#a1a1aa);">同期対象: ${a.calendars.length}件</div></div>
          <div style="display:flex;gap:6px;">
            ${a.is_primary ? "" : `<button class="btn btn-secondary btn-sm" data-int-ga-primary="${a.id}">primaryに</button>`}
            <button class="btn btn-secondary btn-sm" data-int-ga-cal="${a.id}">カレンダー</button>
            <button class="btn btn-secondary btn-sm" data-int-ga-del="${a.id}">削除</button>
          </div>`;
				listEl.appendChild(card);
			});
			listEl.querySelectorAll("[data-int-ga-primary]").forEach((btn) =>
				btn.addEventListener("click", async () => {
					await intPost("/api/integrated/google/accounts/primary", {
						accountId: Number(btn.getAttribute("data-int-ga-primary")),
					});
					fetchIntegratedOverview();
				}),
			);
			listEl.querySelectorAll("[data-int-ga-del]").forEach((btn) =>
				btn.addEventListener("click", async () => {
					if (!confirm("このGoogleアカウント連携を解除しますか？")) return;
					await intPost("/api/integrated/google/accounts/delete", {
						accountId: Number(btn.getAttribute("data-int-ga-del")),
					});
					fetchIntegratedOverview();
				}),
			);
			listEl
				.querySelectorAll("[data-int-ga-cal]")
				.forEach((btn) =>
					btn.addEventListener("click", () =>
						intEditCalendars(Number(btn.getAttribute("data-int-ga-cal"))),
					),
				);
		}
		// Bot別 使用アカウント（system_default は対象外）。primary既定 / 連携なし / 特定アカウント の3択。
		if (assignEl) {
			assignEl.innerHTML = "";
			const options = accounts
				.map(
					(a) =>
						`<option value="acct:${a.id}">${intEsc(a.email || "アカウント#" + a.id)}</option>`,
				)
				.join("");
			const owned = bots.filter((b) => !b.is_system_default);
			owned.forEach((b) => {
				const row = document.createElement("div");
				row.style.cssText =
					"display:flex;align-items:center;justify-content:space-between;gap:10px;";
				row.innerHTML = `<span style="font-size:0.85rem;">${intEsc(b.name)}</span>
          <select data-int-ga-assign="${b.id}" style="min-width:200px;">
            <option value="primary">（primaryを使用）</option>
            <option value="none">連携なし</option>
            ${options}
          </select>`;
				assignEl.appendChild(row);
				const sel = row.querySelector("select");
				const gs = b.google_setting;
				sel.value = gs === "primary" || gs === "none" ? gs : "acct:" + gs;
				sel.addEventListener("change", async () => {
					const v = sel.value;
					let body;
					if (v === "primary") body = { botId: b.id, mode: "primary" };
					else if (v === "none") body = { botId: b.id, mode: "none" };
					else
						body = {
							botId: b.id,
							mode: "account",
							accountId: Number(v.slice(5)),
						};
					const d = await intPost("/api/integrated/grants/google", body);
					if (!d.success && d.message) {
						alert(d.message);
						fetchIntegratedOverview();
					}
				});
			});
			if (!owned.length)
				assignEl.innerHTML =
					'<p class="description-text">所有Botがありません。</p>';
		}
	}

	// カレンダー同期設定をモーダルで編集（prompt の置き換え）。
	// 利用可能なカレンダーをチェックボックス一覧で表示し、選択分を同期対象として保存する。
	async function intEditCalendars(accountId) {
		const modal = document.getElementById("modal-int-calendars");
		const listEl = document.getElementById("int-cal-list");
		const labelEl = document.getElementById("int-cal-account-label");
		const errEl = document.getElementById("int-cal-error");
		const saveBtn = document.getElementById("int-cal-save");
		if (!modal || !listEl || !saveBtn) return;

		const acct = (intOverview?.googleAccounts || []).find(
			(a) => a.id === accountId,
		);
		const current = new Set(acct?.calendars || []);
		if (labelEl)
			labelEl.textContent = `アカウント: ${acct?.email || "#" + accountId}`;
		if (errEl) {
			errEl.style.display = "none";
			errEl.textContent = "";
		}
		listEl.innerHTML = '<p class="description-text">読み込み中…</p>';
		saveBtn.disabled = true;
		openModal(modal);

		let avail = [];
		try {
			const r = await (
				await fetch(`/api/integrated/google/accounts/${accountId}/calendars`)
			).json();
			avail = r.calendars || [];
		} catch (e) {
			avail = [];
		}

		if (!avail.length) {
			listEl.innerHTML =
				'<p class="description-text">カレンダーを取得できませんでした。アカウントの連携状態を確認してください。</p>';
		} else {
			listEl.innerHTML = "";
			avail.forEach((c) => {
				const label = document.createElement("label");
				label.className = "glass";
				label.style.cssText =
					"display:flex;align-items:center;gap:10px;padding:10px 12px;border-radius:8px;cursor:pointer;";
				const cb = document.createElement("input");
				cb.type = "checkbox";
				cb.value = c.id;
				cb.checked = current.has(c.id);
				cb.style.cssText = "width:16px;height:16px;flex-shrink:0;";
				const text = document.createElement("div");
				text.style.cssText = "min-width:0;";
				text.innerHTML = `<div style="font-size:0.9rem;">${intEsc(c.summary || c.id)}</div>
          <div style="font-size:0.72rem;color:var(--color-zinc-muted,#a1a1aa);word-break:break-all;">${intEsc(c.id)}</div>`;
				label.appendChild(cb);
				label.appendChild(text);
				listEl.appendChild(label);
			});
			saveBtn.disabled = false;
		}

		// 開くたびに最新の accountId で保存処理を差し替える（リスナー重複を防ぐ）。
		saveBtn.onclick = async () => {
			const calendars = [...listEl.querySelectorAll('input[type="checkbox"]')]
				.filter((cb) => cb.checked)
				.map((cb) => cb.value);
			saveBtn.disabled = true;
			if (errEl) {
				errEl.style.display = "none";
				errEl.textContent = "";
			}
			try {
				const d = await intPost("/api/integrated/google/accounts/calendars", {
					accountId,
					calendars,
				});
				if (d && d.success === false) {
					if (errEl) {
						errEl.textContent = d.message || "保存に失敗しました。";
						errEl.style.display = "block";
					}
					saveBtn.disabled = false;
					return;
				}
				closeModal(modal);
				fetchIntegratedOverview();
			} catch (e) {
				if (errEl) {
					errEl.textContent = "通信エラーが発生しました。";
					errEl.style.display = "block";
				}
				saveBtn.disabled = false;
			}
		};
	}

	function wireIntegratedForms() {
		intWired = true;
		// 画面のどこかをクリックしたら開いている ⋮ メニューを閉じる（一度きり登録）。
		document.addEventListener("click", closeIntMenus);
		const refresh = document.getElementById("int-refresh");
		if (refresh) refresh.addEventListener("click", fetchIntegratedOverview);

		const credForm = document.getElementById("int-cred-form");
		if (credForm)
			credForm.addEventListener("submit", async (e) => {
				e.preventDefault();
				const payload = {
					serviceName: document.getElementById("int-cred-service").value.trim(),
					username: document.getElementById("int-cred-username").value.trim(),
					password: document.getElementById("int-cred-password").value,
					url:
						document.getElementById("int-cred-url").value.trim() || undefined,
				};
				const d = await intPost("/api/credentials/register", payload);
				alert(
					d.message || (d.success ? "登録しました。" : "登録に失敗しました。"),
				);
				if (d.success) {
					credForm.reset();
					fetchIntegratedOverview();
				}
			});

		const mcpForm = document.getElementById("int-mcp-form");
		if (mcpForm)
			mcpForm.addEventListener("submit", async (e) => {
				e.preventDefault();
				const payload = {
					name: document.getElementById("int-mcp-name").value.trim(),
					endpointUrl: document.getElementById("int-mcp-endpoint").value.trim(),
					requiresConfirmation:
						document.getElementById("int-mcp-confirm").checked,
				};
				const auth = document.getElementById("int-mcp-auth").value;
				if (auth) payload.authCredential = auth;
				if (activeUserRole === "admin")
					payload.scope =
						document.getElementById("int-mcp-scope").value === "system"
							? "system"
							: "user";
				const d = await intPost("/api/mcp-servers/add", payload);
				alert(
					d.message || (d.success ? "登録しました。" : "登録に失敗しました。"),
				);
				if (d.success) {
					mcpForm.reset();
					document.getElementById("int-mcp-confirm").checked = true;
					fetchIntegratedOverview();
				}
			});

		const gConnect = document.getElementById("int-google-connect");
		if (gConnect)
			gConnect.addEventListener("click", async () => {
				try {
					const r = await (
						await fetch("/api/settings/google/oauth/url")
					).json();
					if (r.url) window.location.href = r.url;
				} catch (e) {
					alert("OAuth URLの取得に失敗しました。");
				}
			});
	}

	// Handle popstate event (back/forward browser buttons)
	window.addEventListener("popstate", () => {
		applyRoute(window.location.pathname);
	});

	// On page load, try auto-login
	initAppSession();

	// Service Worker 登録
	if ("serviceWorker" in navigator) {
		navigator.serviceWorker
			.register("/admin/sw.js", { scope: "/admin/" })
			.catch(() => {});
	}
});
