// deviceApi — user-scoped（scope:'user'）。
// device-management（deviceMgmtRoutes.ts）+ device-auth 承認（deviceAuthRoutes.ts）。
//
// ★重要: /api/auth/device/* と device-management 3本は botId を「絶対に付けない」
//   （§10.1。stale な botId を混ぜない）。OAuth トークンポーリング（pollToken）は
//   エンベロープ外の専用実装（../device.ts）を再輸出する。
import { api } from "../client";
import { pollToken, requestDeviceCode } from "../device";
import type { ApiResponse } from "../types";

const USER = { scope: "user" } as const;

// deviceMgmtRoutes.ts の実応答形状（types.ts の DeviceRecord は name 形状で不一致のため
// 接続端末タブ用に device_name / current / created_at / last_used_at を明示する）。
export interface DesktopDevice {
	id: number;
	device_name: string;
	created_at: string;
	last_used_at?: string | null;
	current?: boolean;
}
export type DesktopDevicesResponse = ApiResponse & {
	devices?: DesktopDevice[];
};

/** GET /api/desktop/info の応答（配布バイナリのメタ情報）。 */
export type DesktopInfoResponse = ApiResponse & {
	available?: boolean;
	size?: number;
	version?: string;
};

export const deviceApi = {
	// ── デバイス管理 ──
	/** GET /api/devices — 登録済みデバイス一覧（device_name/current 形状） */
	list: () => api.get<DesktopDevicesResponse>("/api/devices", USER),
	/** POST /api/devices/revoke — デバイス失効（サーバは body.id:number を読む） */
	revoke: (id: number) =>
		api.post<ApiResponse>("/api/devices/revoke", { id }, USER),

	// ── デスクトップ配布 ──
	/** GET /api/desktop/info — Windows 版バイナリの配布状況 */
	desktopInfo: () => api.get<DesktopInfoResponse>("/api/desktop/info", USER),

	// ── デバイス認可（ブラウザ側の承認操作） ──
	/** POST /api/auth/device/approve — user_code をログイン済み本人が承認 */
	approve: (userCode: string) =>
		api.post<ApiResponse & { device_name?: string }>(
			"/api/auth/device/approve",
			{ user_code: userCode },
			USER,
		),

	// ── OAuth device flow（エンベロープ外・専用実装を再輸出。§10.4） ──
	/** POST /api/auth/device/code — デバイスコード発行 */
	requestDeviceCode,
	/** POST /api/auth/device/token — トークンポーリング（OAuth 形状） */
	pollToken,
};
