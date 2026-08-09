// UIモック用 fetch スタブ。/api/* をインメモリ状態で応答し、それ以外は素通しする。
// BotDiscord タブが叩く endpoint（assistant-config / assistant/* / member-requests）のみ
// 実装し、add/remove/decide は状態を実際に書き換えるので画面上の操作も一通り試せる。
import type {
	AssistantChannel,
	AssistantGuild,
	AssistantMember,
	AssistantRole,
	GuildOptionItem,
	MemberRequest,
} from "../routes/config/configTypes";

const state = {
	guilds: [
		{ guild_id: "111111111111111111", guild_name: "Kawaii Music 本部" },
		{ guild_id: "222222222222222222", guild_name: "開発検証サーバー" },
		{ guild_id: "333333333333333333", guild_name: null }, // 名前解決不能（Bot未参加）ケース
	] as AssistantGuild[],
	channels: [
		{ guild_id: "111111111111111111", channel_id: "444444444444444444", channel_name: "雑談" },
		{ guild_id: "222222222222222222", channel_id: "555555555555555555", channel_name: null },
	] as AssistantChannel[],
	mutedChannels: [
		{ guild_id: "222222222222222222", channel_id: "666666666666666666", channel_name: "運営専用" },
	] as AssistantChannel[],
	members: [
		{ guild_id: "111111111111111111", user_id: "600000000000000001", member_name: "すずね" },
		{ guild_id: "111111111111111111", user_id: "600000000000000002", member_name: "Komorida" },
		{ guild_id: "222222222222222222", user_id: "600000000000000003", member_name: null },
	] as AssistantMember[],
	roles: [
		{ guild_id: "111111111111111111", role_id: "700000000000000001", role_name: "スタッフ" },
		{ guild_id: "222222222222222222", role_id: "700000000000000002", role_name: null },
	] as AssistantRole[],
	requests: [
		{ id: 1, user_id: "600000000000000009", guild_id: "111111111111111111", note: "リスナー枠で使いたいです" },
		{ id: 2, user_id: "600000000000000010", guild_id: "222222222222222222", note: null },
	] as MemberRequest[],
	notes: {
		"111111111111111111": "・配信告知は #お知らせ に流す\n・敬称は「さん」で統一",
	} as Record<string, string>,
};

// ギルドごとのプルダウン候補（available=false ケースは 333... で再現）
const optionMembers: Record<string, GuildOptionItem[]> = {
	"111111111111111111": [
		{ id: "600000000000000001", name: "すずね" },
		{ id: "600000000000000002", name: "Komorida" },
		{ id: "600000000000000004", name: "yuki_dev" },
	],
	"222222222222222222": [{ id: "600000000000000003", name: "test-user" }],
};
const optionRoles: Record<string, GuildOptionItem[]> = {
	"111111111111111111": [
		{ id: "700000000000000001", name: "スタッフ" },
		{ id: "700000000000000003", name: "モデレーター" },
	],
	"222222222222222222": [{ id: "700000000000000002", name: "tester" }],
};

function lookupName(options: Record<string, GuildOptionItem[]>, guildId: string, id: string): string | null {
	return options[guildId]?.find((o) => o.id === id)?.name ?? null;
}

// biome-ignore lint/suspicious/noExplicitAny: モック応答は形状自由
function route(url: URL, method: string, body: any): unknown {
	const p = url.pathname;
	const q = (k: string) => url.searchParams.get(k) ?? "";

	if (p === "/api/bots/assistant-config") {
		return {
			success: true,
			guilds: state.guilds,
			channels: state.channels,
			muted_channels: state.mutedChannels,
			members: state.members,
			roles: state.roles,
		};
	}
	if (p === "/api/bots/assistant/guilds" && method === "POST") {
		if (body.action === "add") {
			if (!state.guilds.some((g) => g.guild_id === body.guildId))
				state.guilds.push({ guild_id: body.guildId, guild_name: null });
		} else state.guilds = state.guilds.filter((g) => g.guild_id !== body.guildId);
		return { success: true };
	}
	if (p === "/api/bots/assistant/channels" && method === "POST") {
		if (body.action === "add")
			state.channels.push({ guild_id: body.guildId, channel_id: body.channelId, channel_name: null });
		else
			state.channels = state.channels.filter(
				(c) => !(c.guild_id === body.guildId && c.channel_id === body.channelId),
			);
		return { success: true };
	}
	if (p === "/api/bots/assistant/muted-channels" && method === "POST") {
		if (body.action === "add")
			state.mutedChannels.push({ guild_id: body.guildId, channel_id: body.channelId, channel_name: null });
		else
			state.mutedChannels = state.mutedChannels.filter(
				(c) => !(c.guild_id === body.guildId && c.channel_id === body.channelId),
			);
		return { success: true };
	}
	if (p === "/api/bots/assistant/members" && method === "POST") {
		if (body.action === "add")
			state.members.push({
				guild_id: body.guildId,
				user_id: body.userId,
				member_name: lookupName(optionMembers, body.guildId, body.userId),
			});
		else
			state.members = state.members.filter(
				(m) => !(m.guild_id === body.guildId && m.user_id === body.userId),
			);
		return { success: true };
	}
	if (p === "/api/bots/assistant/roles" && method === "POST") {
		if (body.action === "add")
			state.roles.push({
				guild_id: body.guildId,
				role_id: body.roleId,
				role_name: lookupName(optionRoles, body.guildId, body.roleId),
			});
		else
			state.roles = state.roles.filter(
				(r) => !(r.guild_id === body.guildId && r.role_id === body.roleId),
			);
		return { success: true };
	}
	if (p === "/api/bots/assistant/guild-options") {
		const gid = q("guildId");
		const available = gid !== "333333333333333333"; // Bot未参加ギルドの縮退表示を再現
		return {
			success: true,
			available,
			members: available ? (optionMembers[gid] ?? []) : [],
			roles: available ? (optionRoles[gid] ?? []) : [],
		};
	}
	if (p === "/api/bots/assistant/guild-note") {
		if (method === "POST") {
			state.notes[body.guildId] = body.content ?? "";
			return { success: true };
		}
		return { success: true, content: state.notes[q("guildId")] ?? "", max_length: 10000 };
	}
	if (p === "/api/bots/member-requests") {
		return { success: true, requests: state.requests };
	}
	{
		const m = p.match(/^\/api\/bots\/member-requests\/(\d+)\/decide$/);
		if (m && method === "POST") {
			const id = Number(m[1]);
			const req = state.requests.find((r) => r.id === id);
			if (req && body.decision === "approved")
				state.members.push({
					guild_id: req.guild_id,
					user_id: req.user_id,
					member_name: lookupName(optionMembers, req.guild_id, req.user_id),
				});
			state.requests = state.requests.filter((r) => r.id !== id);
			return { success: true };
		}
	}
	// 未実装 endpoint は成功応答（モックでは副作用なし）
	return { success: true };
}

export function installMockApi(): void {
	const realFetch = window.fetch.bind(window);
	window.fetch = async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
		const urlStr =
			typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
		const url = new URL(urlStr, location.origin);
		if (!url.pathname.startsWith("/api/")) return realFetch(input, init);
		const method = (init?.method ?? "GET").toUpperCase();
		let body: unknown = {};
		if (typeof init?.body === "string") {
			try {
				body = JSON.parse(init.body);
			} catch {
				/* 非JSONボディは空扱い */
			}
		}
		const json = route(url, method, body);
		await new Promise((r) => setTimeout(r, 120)); // 実運用の遅延感を軽く再現
		return new Response(JSON.stringify(json), {
			status: 200,
			headers: { "Content-Type": "application/json" },
		});
	};
}
