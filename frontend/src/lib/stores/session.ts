import { derived, writable } from "svelte/store";
import { api } from "$lib/api/client";

// §9.1 認証セッションストア: /api/me の単一の真実
export type SessionUser = {
	discordId: string;
	username: string;
	role: "user" | "admin";
} | null;

export const currentUser = writable<SessionUser>(null);

export const isAdmin = derived(currentUser, (u) => u?.role === "admin");
export const isAuthed = derived(currentUser, (u) => u !== null);

/**
 * 起動時に /api/me を叩き currentUser を埋める（initAppSession 相当）。
 * 未ログイン時の 401 は「匿名」として currentUser=null にするだけで /login へ遷移しない（§9.1・§10.2）。
 * login/logout/register/プロフィール更新後にも呼び直して再取得する。
 */
export async function bootstrapSession(): Promise<SessionUser> {
	try {
		// サーバは { success, user: { discordId, username, role }, ...legalUrls } を返す（ネスト）
		const me = await api.get<{
			user: { discordId: string; username: string; role: "user" | "admin" };
		}>("/api/me", { scope: "user", isBootstrap: true });
		const user: SessionUser = {
			discordId: me.user.discordId,
			username: me.user.username,
			role: me.user.role ?? "user",
		};
		currentUser.set(user);
		return user;
	} catch {
		// 401（未ログイン）含め、取得失敗は匿名扱い。遷移はしない。
		currentUser.set(null);
		return null;
	}
}
