// サーバー（crates/yuuka-gemini/src/client.rs の `ALLOWED_MODELS`）が
// `PUT /api/client/settings` で受け付けるモデルの一覧（issue #39）。
// 一覧を変更する場合は Rust 側と必ず揃えること（crates/yuuka-client-api/tests/settings.rs の
// `pwa_model_list_matches_server_allowlist` が差分を検出する）。
export const GEMINI_MODELS = [
  'gemini-3.5-flash',
  'gemini-3.1-flash-lite',
  'gemini-2.5-pro',
  'gemini-2.5-flash',
  'gemini-2.5-flash-lite',
] as const

// サーバー側の既定モデル（`yuuka_gemini::DEFAULT_MODEL`）。`users.gemini_model` が未設定のときに表示される。
export const DEFAULT_GEMINI_MODEL: (typeof GEMINI_MODELS)[number] = 'gemini-3.1-flash-lite'
