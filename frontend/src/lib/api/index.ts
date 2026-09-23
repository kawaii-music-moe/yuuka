// 型付き API クライアント層のエントリポイント（§10）。
//
// 使い方:
//   import { taskApi, api, ApiError } from "$lib/api";
//   import type { TodoWithSubtasks } from "$lib/api";
//
// - 汎用クライアント: api.get / api.post / api.del（scope 必須）+ ApiError
// - 領域別サービス: authApi / botApi / taskApi / … / deviceApi
// - 型: types.ts の全エクスポート
// - デバイス OAuth: pollToken / requestDeviceCode（エンベロープ外・§10.4）

export type { NoBodyOpts, RequestOpts, Scope } from "./client";
export { ApiError, api } from "./client";
export { pollToken, requestDeviceCode } from "./device";
export * from "./services";
export * from "./types";
