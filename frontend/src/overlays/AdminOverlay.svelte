<script lang="ts">
	// ─────────────────────────────────────────────────────────────────────────
	// 全体管理（Admin）オーバービュー（旧 app.js: fetchAdminData 一式 + #tab-admin）。
	//
	// 移植方針:
	//   - 8 セクションを $effect で並列取得（旧 fetchAdminData の Promise.all）。
	//   - テーブル行は createElement 全廃 → {#each}。ロール/ステータス/招待バッジは既存クラス再利用。
	//   - confirm()/alert() → confirmDialog / pushToast。
	//   - 自己保護（自分の行は操作ボタンを出さず「自分」表示）は旧仕様どおり currentUser.discordId で判定。
	//   - 監査ログは直近5件をインライン、全件は AuditModal（自前ページング）。
	// ─────────────────────────────────────────────────────────────────────────
	import { adminApi, personaApi } from "$lib/api/services";
	import { ApiError } from "$lib/api/client";
	import { pushToast } from "$lib/stores/toast";
	import { confirmDialog } from "$lib/components/ui";
	import { Button, Icon } from "$lib/components/ui";
	import { currentUser } from "$lib/stores/session";
	import ManagementOverlayShell from "./ManagementOverlayShell.svelte";
	import type {
		AdminStats,
		AdminUserView,
		AdminBotView,
		InviteCode,
		AuditLogEntry,
		PublicPersonaView,
	} from "$lib/api/types";

	import RoleBadge from "./admin/RoleBadge.svelte";
	import AuditModal from "./admin/AuditModal.svelte";
	import { maskDiscordId } from "./admin/adminUtils";

	const AUDIT_PREVIEW_LIMIT = 5;

	let stats = $state<AdminStats | null>(null);
	let users = $state<AdminUserView[]>([]);
	let botsList = $state<AdminBotView[]>([]);
	let inviteCodes = $state<InviteCode[]>([]);
	let auditPreview = $state<AuditLogEntry[]>([]);
	let personas = $state<PublicPersonaView[]>([]);

	// フォーム state
	let defaultBotToken = $state("");
	let privacyUrl = $state("");
	let termsUrl = $state("");
	let presetSecretary = $state("");
	let presetMcp = $state("");
	let rateUserMin = $state<number | "">("");
	let rateUserDay = $state<number | "">("");
	let rateGuildDay = $state<number | "">("");
	let inviteCodeInput = $state("");

	let auditOpen = $state(false);

	const selfId = $derived($currentUser?.discordId ?? "");

	function reportError(e: unknown) {
		pushToast(e instanceof ApiError ? e.message : "エラーが発生しました", "error");
	}

	// ── 各セクション取得 ──
	async function loadStats() {
		try {
			stats = (await adminApi.stats()).stats;
		} catch (e) {
			reportError(e);
		}
	}
	async function loadUsers() {
		try {
			users = (await adminApi.users()).users ?? [];
		} catch (e) {
			reportError(e);
		}
	}
	async function loadBots() {
		try {
			botsList = (await adminApi.bots()).bots ?? [];
		} catch (e) {
			reportError(e);
		}
	}
	async function loadInvites() {
		try {
			inviteCodes = (await adminApi.inviteCodes()).codes ?? [];
		} catch (e) {
			reportError(e);
		}
	}
	async function loadSystemSettings() {
		try {
			const s = await adminApi.systemSettings();
			privacyUrl = s.privacyPolicyUrl || "";
			termsUrl = s.termsUrl || "";
		} catch (e) {
			reportError(e);
		}
	}
	async function loadAuditPreview() {
		try {
			const res = await adminApi.auditLogs({ limit: AUDIT_PREVIEW_LIMIT });
			auditPreview = res.logs ?? [];
		} catch (e) {
			reportError(e);
		}
	}
	async function loadPersonas() {
		try {
			personas = (await personaApi.marketplace()).personas ?? [];
		} catch (e) {
			reportError(e);
		}
	}
	async function loadBotAttrSettings() {
		try {
			const res = await adminApi.botAttributeSettings();
			const byId = new Map(res.presets.map((p) => [p.id, p]));
			presetSecretary = byId.get("secretary")?.displayName ?? "";
			presetMcp = byId.get("mcp_assistant")?.displayName ?? "";
			const rl = res.rate_limits ?? {};
			rateUserMin = rl.userPerMinute ?? 5;
			rateUserDay = rl.userPerDay ?? 100;
			rateGuildDay = rl.guildPerDay ?? 1000;
		} catch (e) {
			reportError(e);
		}
	}

	// 初回に全セクションを並列取得（旧 fetchAdminData）。
	$effect(() => {
		void loadStats();
		void loadUsers();
		void loadBots();
		void loadInvites();
		void loadSystemSettings();
		void loadAuditPreview();
		void loadPersonas();
		void loadBotAttrSettings();
	});

	// ── ユーザー: ロール変更 / 削除 ──
	async function toggleRole(user: AdminUserView) {
		const newRole = user.role === "admin" ? "user" : "admin";
		const ok = await confirmDialog({
			message:
				newRole === "admin"
					? `ユーザー「${user.username}」を Admin に昇格しますか？`
					: `ユーザー「${user.username}」の Admin 権限を解除しますか？`,
			confirmLabel: newRole === "admin" ? "昇格" : "降格",
		});
		if (!ok) return;
		try {
			await adminApi.setUserRole({ targetUserId: user.discord_id, role: newRole });
			await loadUsers();
			await loadStats();
		} catch (e) {
			reportError(e);
		}
	}
	async function deleteUser(user: AdminUserView) {
		const ok = await confirmDialog({
			message: `ユーザー「${user.username}」を完全に削除しますか？\nタスク・家計簿・ペルソナ等の関連データも全て削除され、元に戻せません。`,
			danger: true,
			confirmLabel: "削除",
		});
		if (!ok) return;
		try {
			const res = await adminApi.deleteUser(user.discord_id);
			if (res.message) pushToast(res.message, "success");
			await loadUsers();
			await loadStats();
			await loadBots();
		} catch (e) {
			reportError(e);
		}
	}

	// ── Bot: 差し押さえ / 解除 ──
	async function suspendBot(bot: AdminBotView) {
		const ok = await confirmDialog({
			message: `Bot「${bot.name}」を差し押さえますか？\nDiscordクライアントが停止され、所有者は再起動できなくなります。`,
			danger: true,
			confirmLabel: "差し押さえ",
		});
		if (!ok) return;
		try {
			await adminApi.suspendBot(bot.id);
			await loadBots();
			await loadStats();
		} catch (e) {
			reportError(e);
		}
	}
	async function unsuspendBot(bot: AdminBotView) {
		const ok = await confirmDialog({
			message: `Bot「${bot.name}」の差し押さえを解除しますか？`,
			confirmLabel: "解除",
		});
		if (!ok) return;
		try {
			await adminApi.unsuspendBot(bot.id);
			await loadBots();
			await loadStats();
		} catch (e) {
			reportError(e);
		}
	}

	// ── 招待コード: 作成 / 無効化 / 削除 ──
	async function createInvite(e: SubmitEvent) {
		e.preventDefault();
		const code = inviteCodeInput.trim();
		if (!code) return;
		try {
			await adminApi.createInviteCode(code);
			inviteCodeInput = "";
			await loadInvites();
			await loadStats();
		} catch (err) {
			reportError(err);
		}
	}
	async function revokeInvite(code: InviteCode) {
		const ok = await confirmDialog({
			message: `招待コード「${code.code}」を無効化しますか？\n記録は残りますが、登録には使用できなくなります。`,
			confirmLabel: "無効化",
		});
		if (!ok) return;
		try {
			await adminApi.revokeInviteCode(code.code);
			await loadInvites();
			await loadStats();
		} catch (e) {
			reportError(e);
		}
	}
	async function deleteInvite(code: InviteCode) {
		const ok = await confirmDialog({
			message: `招待コード「${code.code}」を完全に削除しますか？\nこの操作は元に戻せません。`,
			danger: true,
			confirmLabel: "削除",
		});
		if (!ok) return;
		try {
			await adminApi.deleteInviteCode(code.code);
			await loadInvites();
			await loadStats();
		} catch (e) {
			reportError(e);
		}
	}

	// ── ペルソナ: 非公開化 / 削除 ──
	async function unpublishPersona(p: PublicPersonaView) {
		const ok = await confirmDialog({
			message: `ペルソナ「${p.name}」を非公開化しますか？`,
			confirmLabel: "非公開化",
		});
		if (!ok) return;
		try {
			await adminApi.unpublishPersona(p.id);
			await loadPersonas();
		} catch (e) {
			reportError(e);
		}
	}
	async function deletePersona(p: PublicPersonaView) {
		const ok = await confirmDialog({
			message: `ペルソナ「${p.name}」を完全に削除しますか？`,
			danger: true,
			confirmLabel: "削除",
		});
		if (!ok) return;
		try {
			await adminApi.deletePersona(p.id);
			await loadPersonas();
		} catch (e) {
			reportError(e);
		}
	}

	// ── フォーム: デフォルトBotトークン / システム設定 / Bot属性設定 ──
	async function submitDefaultBotToken(e: SubmitEvent) {
		e.preventDefault();
		const token = defaultBotToken.trim();
		if (!token) return;
		try {
			await adminApi.setDefaultBotToken({ token });
			defaultBotToken = "";
			pushToast("システムデフォルト Bot のトークンを更新しました！", "success");
		} catch (err) {
			reportError(err);
		}
	}
	async function submitSystemSettings(e: SubmitEvent) {
		e.preventDefault();
		try {
			await adminApi.saveSystemSettings({
				privacyPolicyUrl: privacyUrl.trim(),
				termsUrl: termsUrl.trim(),
			});
			pushToast("システム設定を保存しました。", "success");
		} catch (err) {
			reportError(err);
		}
	}
	async function submitBotAttr(e: SubmitEvent) {
		e.preventDefault();
		try {
			await adminApi.saveBotAttributeSettings({
				displayNames: {
					secretary: presetSecretary.trim(),
					mcp_assistant: presetMcp.trim(),
				},
				rateLimits: {
					userPerMinute: Number(rateUserMin),
					userPerDay: Number(rateUserDay),
					guildPerDay: Number(rateGuildDay),
				},
			});
			pushToast("Bot属性設定を保存しました。", "success");
			await loadBotAttrSettings();
		} catch (err) {
			reportError(err);
		}
	}

	function botStatus(bot: AdminBotView): { label: string; tone: string } {
		if (bot.suspended)
			return { label: "差し押さえ中", tone: "status-suspended" };
		if (bot.hasCustomToken)
			return { label: bot.isRunning ? "起動中" : "停止", tone: "status-active" };
		return { label: "デフォルト", tone: "status-default" };
	}
</script>

<ManagementOverlayShell id="admin-overlay" icon="admin_panel_settings" title="管理者設定">
<section id="tab-admin" class="tab-view">
	<!-- KPI Row -->
	<div class="admin-kpi-row">
		<div class="admin-kpi">
			<span class="admin-kpi-value">{stats?.totalUsers ?? 0}</span>
			<span class="admin-kpi-label"><Icon name="group" size={14} />ユーザー</span>
		</div>
		<div class="admin-kpi-sep"></div>
		<div class="admin-kpi">
			<span class="admin-kpi-value">{stats?.totalBots ?? 0}</span>
			<span class="admin-kpi-label"><Icon name="robot_2" size={14} />Bot 総数</span>
		</div>
		<div class="admin-kpi-sep"></div>
		<div class="admin-kpi">
			<span class="admin-kpi-value">{stats?.suspendedBots ?? 0}</span>
			<span class="admin-kpi-label"><Icon name="block" size={14} />差し押さえ</span>
		</div>
		<div class="admin-kpi-sep"></div>
		<div class="admin-kpi">
			<span class="admin-kpi-value">{stats?.availableInviteCodes ?? 0}</span>
			<span class="admin-kpi-label"
				><Icon name="confirmation_number" size={14} />招待コード残</span
			>
		</div>
	</div>

	<!-- Default Bot -->
	<details class="admin-section" name="admin-acc">
		<summary class="admin-section-header">
			<span class="admin-section-title"><Icon name="smart_toy" />システムデフォルト Bot</span>
			<span class="hud-tag admin-tag-system">SYSTEM</span>
		</summary>
		<p class="description-text">
			システム共通で動作するデフォルトBotのDiscordトークンを更新します。トークンは安全に暗号化されて保存されます。
		</p>
		<form class="admin-form-top" onsubmit={submitDefaultBotToken}>
			<div class="form-group">
				<label for="admin-default-bot-token">デフォルト Bot トークン *</label>
				<div class="admin-inline-form">
					<input
						type="password"
						id="admin-default-bot-token"
						bind:value={defaultBotToken}
						required
						placeholder="新しい Discord Bot Token を入力"
						autocomplete="new-password"
						class="admin-grow"
					/>
					<Button variant="primary" type="submit">設定を更新</Button>
				</div>
				<span class="field-sub"
					>※更新すると、デフォルトBotは自動的に新しいトークンで再起動します。</span
				>
			</div>
		</form>
	</details>

	<!-- System Settings -->
	<details class="admin-section" name="admin-acc">
		<summary class="admin-section-header">
			<span class="admin-section-title"><Icon name="settings" />システム全体設定</span>
			<span class="hud-tag admin-tag-system">SYSTEM</span>
		</summary>
		<p class="description-text">システム全体の共通設定を行います。</p>
		<form class="admin-form-top" onsubmit={submitSystemSettings}>
			<div class="form-group">
				<label for="admin-privacy-policy-url"
					>一般公開プライバシーポリシーへのリンク (URL / パス)</label
				>
				<input
					type="text"
					id="admin-privacy-policy-url"
					bind:value={privacyUrl}
					placeholder="https://example.com/privacy または /privacy"
				/>
				<span class="field-sub"
					>※空欄にすると、ログイン画面および使い方ガイドからプライバシーポリシーのリンクが非表示になります。</span
				>
			</div>
			<div class="form-group admin-mt">
				<label for="admin-terms-url">一般公開利用規約へのリンク (URL / パス)</label>
				<input
					type="text"
					id="admin-terms-url"
					bind:value={termsUrl}
					placeholder="https://example.com/terms または /terms"
				/>
				<span class="field-sub"
					>※空欄にすると、ログイン画面および使い方ガイドから利用規約のリンクが非表示になります。</span
				>
			</div>
			<Button variant="primary" type="submit" class="admin-mt">設定を更新</Button>
		</form>
	</details>

	<!-- Bot Attribute Settings -->
	<details class="admin-section" name="admin-acc">
		<summary class="admin-section-header">
			<span class="admin-section-title"><Icon name="category" />Bot属性設定</span>
			<span class="hud-tag admin-tag-system">SYSTEM</span>
		</summary>
		<p class="description-text">
			Botプリセットのユーザー向け表示名と、汎用モード（MCPアシスタント）のレート制限既定値を設定します。内部IDは固定です。
		</p>
		<form class="admin-form-top" onsubmit={submitBotAttr}>
			<div class="form-row">
				<div class="form-group">
					<label for="admin-preset-name-secretary">「secretary」の表示名</label>
					<input
						type="text"
						id="admin-preset-name-secretary"
						bind:value={presetSecretary}
						placeholder="パーソナル秘書"
						maxlength="50"
					/>
				</div>
				<div class="form-group">
					<label for="admin-preset-name-mcp">「mcp_assistant」の表示名</label>
					<input
						type="text"
						id="admin-preset-name-mcp"
						bind:value={presetMcp}
						placeholder="汎用モード"
						maxlength="50"
					/>
				</div>
			</div>
			<div class="form-row admin-mt-sm">
				<div class="form-group">
					<label for="admin-rate-user-min">ユーザー上限（回/分）</label>
					<input type="number" id="admin-rate-user-min" bind:value={rateUserMin} min="1" placeholder="5" />
				</div>
				<div class="form-group">
					<label for="admin-rate-user-day">ユーザー上限（回/日）</label>
					<input type="number" id="admin-rate-user-day" bind:value={rateUserDay} min="1" placeholder="100" />
				</div>
				<div class="form-group">
					<label for="admin-rate-guild-day">ギルド上限（回/日）</label>
					<input type="number" id="admin-rate-guild-day" bind:value={rateGuildDay} min="1" placeholder="1000" />
				</div>
			</div>
			<Button variant="primary" type="submit" class="admin-mt">Bot属性設定を保存</Button>
		</form>
	</details>

	<!-- User Management -->
	<details class="admin-section" name="admin-acc">
		<summary class="admin-section-header">
			<span class="admin-section-title"><Icon name="manage_accounts" />ユーザー管理</span>
			<span class="hud-tag">ADMIN</span>
		</summary>
		<p class="description-text">
			登録済みユーザーの一覧とロール管理を行います。個人情報（パスワード等）は表示されません。
		</p>
		<div class="table-responsive admin-mt">
			<table class="expense-table admin-table-full">
				<thead>
					<tr>
						<th class="admin-table-th">ユーザー名</th>
						<th class="admin-table-th">Discord ID</th>
						<th class="admin-table-th">ロール</th>
						<th class="admin-table-th">登録日</th>
						<th class="admin-table-th admin-th-actions">操作</th>
					</tr>
				</thead>
				<tbody>
					{#if users.length === 0}
						<tr><td colspan="5" class="admin-empty">登録済みユーザーはいません。</td></tr>
					{:else}
						{#each users as user (user.discord_id)}
							<tr>
								<td class="admin-table-td admin-td-name">{user.username}</td>
								<td class="admin-table-td admin-discord-id">{maskDiscordId(user.discord_id)}</td>
								<td class="admin-table-td"><RoleBadge role={user.role} /></td>
								<td class="admin-table-td admin-td-date">{user.created_at}</td>
								<td class="admin-table-td admin-td-actions">
									{#if user.discord_id !== selfId}
										<div class="admin-action-group">
											<button
												type="button"
												class="admin-btn-action {user.role === 'admin' ? '' : 'btn-promote'}"
												onclick={() => toggleRole(user)}
												>{user.role === "admin" ? "降格" : "管理者に"}</button
											>
											<button
												type="button"
												class="admin-btn-action btn-danger"
												onclick={() => deleteUser(user)}>削除</button
											>
										</div>
									{:else}
										<span class="admin-self">自分</span>
									{/if}
								</td>
							</tr>
						{/each}
					{/if}
				</tbody>
			</table>
		</div>
	</details>

	<!-- Bot Moderation -->
	<details class="admin-section" name="admin-acc">
		<summary class="admin-section-header">
			<span class="admin-section-title"><Icon name="security" />Bot モデレーション</span>
			<span class="hud-tag">MODERATION</span>
		</summary>
		<p class="description-text">
			システム上の全Botの管理・差し押さえを行います。差し押さえるとBotのDiscordクライアントが停止され、所有者は再起動できなくなります。
		</p>
		<div class="table-responsive admin-mt">
			<table class="expense-table admin-table-full">
				<thead>
					<tr>
						<th class="admin-table-th">Bot名</th>
						<th class="admin-table-th">所有者</th>
						<th class="admin-table-th">ステータス</th>
						<th class="admin-table-th">作成日</th>
						<th class="admin-table-th admin-th-actions">操作</th>
					</tr>
				</thead>
				<tbody>
					{#if botsList.length === 0}
						<tr><td colspan="5" class="admin-empty">Botは登録されていません。</td></tr>
					{:else}
						{#each botsList as bot (bot.id)}
							{@const st = botStatus(bot)}
							<tr>
								<td class="admin-table-td admin-td-name">{bot.discord_username || bot.name}</td>
								<td class="admin-table-td">{bot.owner_username}</td>
								<td class="admin-table-td"
									><span class="admin-status-badge {st.tone}">{st.label}</span></td
								>
								<td class="admin-table-td admin-td-date">{bot.created_at}</td>
								<td class="admin-table-td admin-td-actions">
									{#if bot.suspended}
										<button
											type="button"
											class="admin-btn-action btn-success"
											onclick={() => unsuspendBot(bot)}>解除</button
										>
									{:else}
										<button
											type="button"
											class="admin-btn-action btn-danger"
											onclick={() => suspendBot(bot)}>差し押さえ</button
										>
									{/if}
								</td>
							</tr>
						{/each}
					{/if}
				</tbody>
			</table>
		</div>
	</details>

	<!-- Persona Marketplace Moderation -->
	<details class="admin-section" name="admin-acc">
		<summary class="admin-section-header">
			<span class="admin-section-title"
				><Icon name="storefront" />ペルソナ マーケットプレイス管理</span
			>
			<span class="hud-tag">MODERATION</span>
		</summary>
		<p class="description-text">
			マーケットプレイスに公開されている全ペルソナの非公開化・削除を行います。
		</p>
		<div class="table-responsive admin-mt">
			<table class="expense-table admin-table-full">
				<thead>
					<tr>
						<th class="admin-table-th">ID</th>
						<th class="admin-table-th">名前</th>
						<th class="admin-table-th">作成者</th>
						<th class="admin-table-th">文字数</th>
						<th class="admin-table-th admin-th-actions">操作</th>
					</tr>
				</thead>
				<tbody>
					{#if personas.length === 0}
						<tr><td colspan="5" class="admin-empty">公開中のペルソナはありません。</td></tr>
					{:else}
						{#each personas as p (p.id)}
							<tr>
								<td class="admin-table-td admin-mono">{p.id}</td>
								<td class="admin-table-td admin-td-name">{p.name}</td>
								<td class="admin-table-td">{p.owner_username}</td>
								<td class="admin-table-td admin-mono">{(p.prompt_length || 0).toLocaleString()}</td>
								<td class="admin-table-td admin-td-actions">
									<button
										type="button"
										class="admin-btn-action"
										onclick={() => unpublishPersona(p)}>非公開化</button
									>
									<button
										type="button"
										class="admin-btn-action btn-danger admin-ml"
										onclick={() => deletePersona(p)}>削除</button
									>
								</td>
							</tr>
						{/each}
					{/if}
				</tbody>
			</table>
		</div>
	</details>

	<!-- Audit Logs -->
	<details class="admin-section" name="admin-acc">
		<summary class="admin-section-header">
			<span class="admin-section-title"><Icon name="receipt_long" />監査ログ</span>
			<span class="hud-tag">AUDIT</span>
		</summary>
		<p class="description-text">
			セキュリティ関連の操作履歴です。直近の数件を表示しています。全件の閲覧・絞り込みは「すべて表示」から行えます。
		</p>
		<div class="table-responsive">
			<table class="expense-table admin-table-full">
				<thead>
					<tr>
						<th class="admin-table-th">日時</th>
						<th class="admin-table-th">ユーザー</th>
						<th class="admin-table-th">Action</th>
						<th class="admin-table-th">対象</th>
						<th class="admin-table-th">詳細</th>
					</tr>
				</thead>
				<tbody>
					{#if auditPreview.length === 0}
						<tr><td colspan="5" class="admin-empty">監査ログはありません。</td></tr>
					{:else}
						{#each auditPreview as log (log.id)}
							<tr>
								<td class="admin-table-td admin-audit-date">{log.created_at}</td>
								<td class="admin-table-td admin-discord-id">{maskDiscordId(log.user_id)}</td>
								<td class="admin-table-td admin-audit-action">{log.action}</td>
								<td class="admin-table-td admin-audit-muted">{log.target || "—"}</td>
								<td class="admin-table-td admin-audit-muted">{log.detail || "—"}</td>
							</tr>
						{/each}
					{/if}
				</tbody>
			</table>
		</div>
		<div class="admin-audit-more">
			<Button variant="secondary" onclick={() => (auditOpen = true)}>
				<Icon name="open_in_full" />すべて表示
			</Button>
		</div>
	</details>

	<!-- Invite Codes -->
	<details class="admin-section admin-section-last" name="admin-acc">
		<summary class="admin-section-header">
			<span class="admin-section-title"><Icon name="confirmation_number" />招待コード管理</span>
		</summary>
		<p class="description-text">新規ユーザー登録に必要な招待コードの管理を行います。</p>
		<form class="admin-invite-form" onsubmit={createInvite}>
			<input
				type="text"
				bind:value={inviteCodeInput}
				required
				placeholder="新しい招待コードを入力"
				class="admin-grow"
			/>
			<Button variant="primary" type="submit">コード作成</Button>
		</form>
		<div class="table-responsive">
			<table class="expense-table admin-table-full">
				<thead>
					<tr>
						<th class="admin-table-th">コード</th>
						<th class="admin-table-th">ステータス</th>
						<th class="admin-table-th">使用者</th>
						<th class="admin-table-th">作成日</th>
						<th class="admin-table-th">操作</th>
					</tr>
				</thead>
				<tbody>
					{#if inviteCodes.length === 0}
						<tr><td colspan="5" class="admin-empty">招待コードは登録されていません。</td></tr>
					{:else}
						{#each inviteCodes as code (code.code)}
							<tr>
								<td class="admin-table-td admin-code">{code.code}</td>
								<td class="admin-table-td">
									{#if code.used_by}
										<span class="invite-status-used">使用済み</span>
									{:else if code.revoked_at}
										<span class="invite-status-revoked">無効</span>
									{:else}
										<span class="invite-status-available">利用可能</span>
									{/if}
								</td>
								<td class="admin-table-td admin-td-muted"
									>{code.used_by ? maskDiscordId(code.used_by) : "—"}</td
								>
								<td class="admin-table-td admin-td-date">{code.created_at}</td>
								<td class="admin-table-td">
									{#if !code.used_by}
										{#if !code.revoked_at}
											<button
												type="button"
												class="admin-btn-action"
												onclick={() => revokeInvite(code)}>無効化</button
											>
										{/if}
										<button
											type="button"
											class="admin-btn-action btn-danger admin-ml"
											onclick={() => deleteInvite(code)}>削除</button
										>
									{:else}
										<span class="admin-td-muted">—</span>
									{/if}
								</td>
							</tr>
						{/each}
					{/if}
				</tbody>
			</table>
		</div>
	</details>
</section>
</ManagementOverlayShell>

<AuditModal bind:open={auditOpen} />

<style>
	.admin-table-full {
		width: 100%;
	}
	.admin-th-actions {
		text-align: right;
		width: 160px;
	}
	.admin-empty {
		text-align: center;
		padding: 20px;
		color: var(--color-zinc-muted);
		font-size: 0.8rem;
	}
	.admin-td-name {
		font-weight: 700;
		color: var(--color-white);
	}
	.admin-td-date {
		font-size: 0.8rem;
		color: var(--color-zinc-muted);
	}
	.admin-td-muted,
	.admin-audit-muted {
		color: var(--color-zinc-muted);
	}
	.admin-td-actions {
		text-align: right;
	}
	.admin-self {
		font-size: 0.75rem;
		color: var(--color-zinc-muted);
	}
	.admin-mono,
	.admin-code {
		font-family: var(--font-family-mono);
	}
	.admin-code {
		font-weight: 600;
	}
	.admin-ml {
		margin-left: 6px;
	}
	.admin-mt {
		margin-top: 16px;
	}
	.admin-mt-sm {
		margin-top: 12px;
	}
	.admin-form-top {
		margin-top: 16px;
	}
	.admin-inline-form {
		display: flex;
		gap: 12px;
		margin-top: 8px;
	}
	.admin-grow {
		flex-grow: 1;
	}
	.admin-invite-form {
		display: flex;
		gap: 12px;
		margin-top: 16px;
		margin-bottom: 20px;
	}
	.admin-audit-date,
	.admin-audit-muted,
	.admin-audit-action {
		font-size: 0.78rem;
	}
	.admin-audit-date {
		white-space: nowrap;
		color: var(--color-zinc-muted);
	}
	.admin-audit-action {
		font-family: var(--font-family-mono);
	}
	.admin-audit-more {
		display: flex;
		justify-content: flex-end;
		margin-top: 16px;
	}
	.admin-section-last {
		border-bottom: none;
	}
	.admin-tag-system {
		background-color: rgba(59, 130, 246, 0.15);
		border-color: rgba(59, 130, 246, 0.4);
		color: #60a5fa;
	}
</style>
