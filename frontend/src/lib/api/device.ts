// ─────────────────────────────────────────────────────────────────────────────
// デバイストークンのポーリング（§10.4・エンベロープ外の専用メソッド）
//
// /api/auth/device/token は OAuth device flow 形状で応答する:
//   - 200 + {access_token,...}                              → 承認完了
//   - 200 + {error:"authorization_pending"|"slow_down"}     → ポーリング継続（正常）
//   - 400/410 + {error}                                     → 失効/不正。停止
//
// 汎用 request() を通すと !res.ok→throw / success 判定で pending/slow_down を誤って
// 握り潰す（200+error を失敗扱いにしてしまう）ため、専用メソッドで OAuth error を
// 検査する。botId は付けない（user-scoped）。
// ─────────────────────────────────────────────────────────────────────────────

import type { DeviceCodeResponse, PollResult } from "./types";

/** POST /api/auth/device/code: デバイスコードの発行を要求（auth:'none'）。 */
export async function requestDeviceCode(
	deviceName?: string,
): Promise<DeviceCodeResponse> {
	const res = await fetch("/api/auth/device/code", {
		method: "POST",
		credentials: "same-origin",
		headers: { "Content-Type": "application/json" },
		body: JSON.stringify(deviceName ? { device_name: deviceName } : {}),
	});
	return (await res.json()) as DeviceCodeResponse;
}

/** POST /api/auth/device/token: device_code をトークンへ交換。OAuth 形状を直接扱う。 */
export async function pollToken(deviceCode: string): Promise<PollResult> {
	const res = await fetch("/api/auth/device/token", {
		method: "POST",
		credentials: "same-origin",
		headers: { "Content-Type": "application/json" },
		// botId は付けない（user-scoped）
		body: JSON.stringify({ device_code: deviceCode }),
	});
	const data = (await res.json().catch(() => ({}))) as {
		access_token?: string;
		error?: string;
	};

	if (res.status === 200 && data.access_token) {
		return { status: "authorized", token: data.access_token };
	}
	if (
		res.status === 200 &&
		(data.error === "authorization_pending" || data.error === "slow_down")
	) {
		return { status: "pending", slowDown: data.error === "slow_down" };
	}
	// 400/410 等（失効・不正）。error コードを surface して停止。
	return { status: "error", code: data.error ?? `http_${res.status}` };
}

export const deviceApi = {
	requestDeviceCode,
	pollToken,
};
