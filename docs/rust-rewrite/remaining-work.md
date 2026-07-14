# Rust 移行 — 残作業ロードマップ（本番投入までの ToDo 全集）

- 最終更新: 2026-07-14（**P2-A/P2-B 18 増分セッション**: admin 15 + settings 6 Web-API（新規 `yuuka-admin`/`yuuka-settings`）+ **todo ツール 4 本 + todo タグ/ガイド 3 本 + finance ツール 9 本**を実装 + 敵対的パリティレビューで確定 0 HIGH。全ゲート緑 — build ✅ / clippy -D ✅ / **test 475** ✅ / deny exit0 ✅・新規依存なし。ルート 120/152・ツール 69/87・**bot 管理系 + Webhook Web-API 完了**）
- 追記（2026-07-15d）: **ペルソナ マーケットプレイス閲覧 Web-API（marketplace 2本）実装**（`yuuka-persona` 拡張・Node `personaRoutes` パリティ）。commit `9f616fd`。GET `/api/personas/marketplace`（`listPublicPersonas`＝`is_public=1` を updated_at 降順・owner は id でなく **owner_username**〔`users` LEFT JOIN・不在「不明」〕）+ GET `/api/personas/marketplace/{id}`（`getPersonaById`+`is_public!==1→404` を `WHERE id=? AND is_public=1` に畳む＝非公開は返さない・整数でない/非公開/不在は 404「公開ペルソナが見つかりません。」・公開は {persona:{id,name,prompt}}）。読み取り専用・auth:user だが owner を跨ぐ公開読み取り（owner_id 非露出）。repo に `list_public`/`get_public` 追加・routes() へ 2 本（既存 merge に自動包含）。**test 499→501**・全ゲート緑・新規依存なし。ルート 124→126/152。**残 personaRoutes**: activate/publish/import/recommended-persona/admin（bot_active_personas/bots/audit 連携）。
- 追記（2026-07-15c）: **デスクトップ OAuth デバイスフロー（RFC 8628・deviceAuthRoutes 3本）実装**（新 `yuuka-auth/src/device_auth.rs`・Node `desktopAuthService.ts` + `deviceAuthRoutes.ts` パリティ）。commit `50992fa`。POST `/api/auth/device/code`（auth:none・device_code/user_code 発行＝Crockford Base32 XXXX-XXXX・verification_uri/complete・interval=5・expires_in=600）/`/approve`（auth:user・本人承認＝400/404/410/200）/`/token`（auth:none・トークン交換＝pending/slow_down=200・expired=410・approved=200 {access_token,Bearer,expires_in,user}）。**device_code 一時状態はインメモリ store**（Node の Redis フォールバック相当＝Redis 非移植・main で 1 度生成し Extension 注入・web 再起動を跨ぐ）、**発行済みトークンのみ SQLite**（`desktop_tokens`・新 `desktop::add_desktop_token`・sha256 保存）。TTL 90 日は既存 `DESKTOP_TOKEN_TTL_DAYS`（verify と一致）。build_app へ device_auth_routes 貫通（webhook 同方式）。**敵対的パリティ+セキュリティレビュー**（3 次元×独立検証・確定 9 件・全 low）で**着地前に是正 3 件**: [security] TOCTOU 二重トークン発行を poll() の**ロック下原子消費**で封じる（Node は DB 書込後削除で窓が空くのを是正）/[parity] 非文字列 body フィールドを accept-and-drop（`as_opt_string`＝Node typeof coercion）/[correctness] user_code 衝突を loop-until-unique（Node の break-after-5 上書き是正）。**意図的 divergence 3 件**: TTL 非 config 化（既存定数と一貫・既定一致）/auth:none への CSRF は Rust の方が厳格（弱めない）/未認証 401 文言は全 auth ルート横断で別件。**test 495→499**・全ゲート緑・新規依存なし。ルート 121→124/152。
- 追記（2026-07-15b）: **API 利用量サマリ Web-API（GET /api/bots/usage）実装**（`yuuka-orchestrator::bot_attr_routes` 拡張・Node `botAttributeRoutes` + `messageLogRepo.getBotUsageSeries` パリティ）。commit `5eaca9c`。message_log.rs に `get_bot_usage_series`（message_logs を bot_id 単位で日次集計＝role=user→requests/assistant→responses・連続日付は**再帰 CTE で SQLite `date('now','localtime')` 生成**＝WHERE と同一基準で TZ ドリフト回避・欠損日 0 補完・days [1,90] クランプ・単一読み取りクエリ）。route は `resolve_scope`（アクセス不可/未指定は system_default フォールバック・読み取り専用ゆえ owner 限定にしない）＋ days の JS `parseInt` 相当（`parse_int_prefix`＝NaN/≤0→14・>90→90）＋ 既存 `rate_limits_json`。応答 `{success,days,series:[{date,requests,responses}],totals:{requests,responses},rate_limits}`。**test 492→495**（repo 集計/欠損補完/クランプ + route shape/クランプ/認証 + 実カウント）・全ゲート緑・新規依存なし。ルート 120→121/152。**残 botAttributeRoutes**: assistant-config（集約 GET＝personas/mcp_servers/usage 束ね・要 persona/mcp list）/guild-options（Discord live）。
- 追記（2026-07-15a）: **認証情報アクセス制御の配線 + 敵対的レビュー是正**（M-10 消費側完了）。commit `d2a7c5b`（feat）+ `767e265`（fix）。**配線 6 点 + crypto 注入**: (1) `CredentialAccessRepo::grant_to_owner_bots`（Node `grantCredentialToOwnerBots`＝owner 所有 Bot ∪ system_default へ冪等付与・**単一 writer Tx で原子的**・owner_id 分離キー）、(2) `routes_with(crypto)` + `CredentialCrypto` Extension（webhook 同方式）＋**新 `POST /api/credentials/register`**（`secretService.registerCredential` パリティ＝raw 必須検証→正規化→salt→暗号化→保存→owner-Bot 付与・失敗は 400 `{success,message}`）、(3) GET `/api/credentials` を `list_credential_names_for_bot` で**許可フィルタ**（未許可 service 非露出）、(4) delete 成功時に `delete_all_grants` 掃除、(5) tool listCredentialServices 許可フィルタ、(6) tool addCredential は保存後に応対 Bot へ grant・deleteCredential は削除後に掃除。`check_field_lengths` を pub(crate) 共有化・`credential_routes = routes_with(crypto)` を build_app/WebService へ貫通。**FK 依存（bot_credential_access.bot_id→bots・foreign_keys=ON）は Node と同一**＝system_default Bot 行は admin セットアップ生成の既存不変（新規退行ではない）。**敵対的パリティレビュー**（4 次元×各指摘を独立ソース検証）で確定 7 件→**5 件是正**: [HIGH] add/update パスワード trim をやめ `arg_password`（サイレント資格情報破損の是正・Node `asOptionalPassword`）/[MED] update `url=""` を URL 削除扱い（`arg_url_update`・Node 別パーサ）/[LOW] delete ガードを raw 空文字のみ 400 + 日本語文言・addCredential 成功文言 full 化。**意図的 divergence 2 件**（是正せず）: register の DB 障害 500/502（クレート横断 DbError→5xx 規約・Node は 400）/crypto 未設定・不正 salt の汎用文言（サーバ構成非露出の安全側）。**test 488→492**・全ゲート緑（build/clippy-D/test/deny）・新規依存なし。ツール 71/87・ルート +1（register）。
- 追記（2026-07-14w）: **認証情報 Bot 許可リポジトリ層（`bot_credential_access`）実装**（M-10 の DB 基盤）。commit `92be802`。Node `credentialAccessRepo.ts` の 6 関数を `CredentialAccessRepo`（`crates/yuuka-credential/src/access.rs`）へ 1:1 移植（SQL バイト単位パリティ）＝grant〔INSERT OR IGNORE 冪等〕/revoke/list_bot_ids_for_credential/list_credential_names_for_bot/is_granted〔ランタイムゲート〕/delete_all_grants。service_name は `normalize_service_name`（trim+小文字化）で照合・保存、owner_id を全クエリ必須キーに（§12.2 契約5）。単体テスト 3。**残（後続増分で配線）**: register ルートの owner-Bot 一括付与 / GET `/api/credentials` の許可フィルタ / delete の grant 掃除 / addCredential ツールの応対 Bot 付与。**test 485→488**・全ゲート緑・新規依存なし。
- 追記（2026-07-14v）: **リッチ返信 Embed ツール（showRichContent）実装**（新クレート `yuuka-richcontent`・Node `richContentModule.ts`/`utils/embeds.ts` パリティ）。commit `a435583`。**Embed 配管を新設**＝(1) core `ResponsePart::Embed(EmbedPart)` + `EmbedPart`/`EmbedFieldPart`、(2) engine `rich_parts_to_embeds`（`ResponsePart::Embed`→discord `RichEmbed`）を秘書/汎用モード両 `TurnReply.embeds` へ設定（従来 embeds 常時空を解消）、(3) ws `done_frame` が embeds を discord.js APIEmbed JSON（title/description/color 十進/fields/footer）へ直列化（desktop `model.rs Embed` 契約一致・従来 embeds 常時空を解消）。color マップ 11 種・title 必須・fields 最大25・name256/value1024 クリップ・rich_reply 無効時は非生成。exposure=core（capability=None・秘書+汎用モード両露出＝Node `richContentModule` 無条件 push）。空テキスト時 FALLBACK_TEXT は Node `gemini.ts:928-930`（embeds 有無に依らず適用）と一致。**敵対的パリティレビュー**（tool-parity/core-engine-plumbing/ws-desktop-discord の 3 次元×各指摘を独立検証）で**確定 0 件**。**意図的 divergence**（対応不要）: clip は char 単位（UTF-16 でなく・安全側・到達不能）/`setTimestamp()` は RichEmbed に該当フィールド無く非付与。**test 477→485**（richcontent 6 + plumbing 2）・全ゲート緑・新規依存なし。ツール 70→71/87。
- 追記（2026-07-14u）: **会話ログ要約ツール（summarizeConversationTopic）実装**（新クレート `yuuka-conversation`・Node `conversationFunctions.ts`/`messageLogRepo.searchMessages` パリティ）。commit `a04a2f9`。FTS5（≥3 字・char-count 閾値）/LIKE（1-2 字・`\%_` エスケープ）/期間のみの 3 経路・全経路 `guild_id IS NULL` + user/bot スコープ（§3.12.3 プライバシー）・11→10 narrowed 判定・時系列 reverse・1000 字 truncate・message 文言 verbatim。能力ゲート `capability="memory"`・secretary のみ（Node `getGuildAssistantFunctionModules` は conversation 非露出＝guild-assistant=false）。`normalize_period` は `replacen(...,1)` で Node `.replace("T"," ")` 先頭のみ置換に一致。**test 475→477**・全ゲート緑・新規依存なし。ツール 69→70/87。
- 追記（2026-07-14e）: **configureBriefing 実装**（yuuka-briefing 拡張・SSRF ガード付き朝報設定ツール）。commit `86ac68f`。**test 440→442**・全ゲート緑・新規依存なし。ツール 68→69/87。
- 追記（2026-07-14f）: **配信設定 Web-API（deliveryRoutes 6 本）実装**（`yuuka-briefing::routes`）。commit `67e0805`。`GET/POST /api/briefing-config`・`POST /api/briefing/test`・`GET/POST /api/report-configs`・`POST /api/report-configs/test`。実配信は **DeliveryRunner シーム**（既定 NullDeliveryRunner＝未配信へ縮退・サービス本体は後続）。repo に find_briefing 追加 + BriefingPatch を target/weather/location のクリア対応へ拡張。**test 442→446**・全ゲート緑・新規依存なし（axum/serde を briefing へ追加のみ）。ルート 80→86/152。
- 追記（2026-07-14g）: **端末管理 Web-API（deviceMgmtRoutes 2 本）実装**（`yuuka-auth::device_routes`）。commit `3a516b5`。`GET /api/devices`（未失効 desktop トークン一覧・Bearer 経路のみ current 判定・トークン本体非返却）・`POST /api/devices/revoke`（soft 失効 revoked=1・不正ID 400・未存在/既失効 404・監査 best-effort）。desktop.rs に list_for_user/revoke + DesktopTokenInfo を追加。**test 446→449**・全ゲート緑・新規依存なし。ルート 86→88/152。
- 追記（2026-07-14t）: **有効モジュール Web-API（modules 2 本 + カタログ）実装**（`bot_attribute_routes` 拡張・新 `module_catalog`）。commit `6c462c6`。GET/POST `/api/bots/modules`（ユーザー個別上書き・解決 override→Bot既定→全有効・null/"all" 解除・既知IDのみ採用）。SELECTABLE_MODULES 14 件 + bot_repo に bot_enabled_modules/get_user_modules/set_user_modules。**test 474→475**・全ゲート緑・新規依存なし。ルート 118→120/152。**残 botAttributeRoutes**: usage/assistant-{config,guild-options〔Discord live〕}。
- 追記（2026-07-14s）: **Bot 専用 Gemini キー Web-API（assistant/gemini-key 1 本）実装**（`bot_attribute_routes` を crypto 注入化）。commit `89135a1`。POST `/api/bots/assistant/gemini-key`（クリア/マスク未変更/形式検証/SystemCrypto 暗号化保存 + 監査）。bot_repo に update_bot_gemini_key。**bot_attribute_routes を build_app 引数化**（webhook と同方式・main で crypto 注入・build_app は too_many_arguments 許容）。**test 473→474**・全ゲート緑・新規依存なし。ルート 117→118/152。**残 botAttributeRoutes**: usage/modules/assistant-{config,guild-options〔Discord live〕}。
- 追記（2026-07-14r）: **汎用モード許可リスト Web-API（assistant guilds/members/roles 3 本）実装**（`bot_attribute_routes` 拡張）。commit `d908a21`。POST `/api/bots/assistant/{guilds,members,roles}`（snowflake 検証 + add/remove + 更新後一覧 + 監査）。bot_repo に BotGuildRow/MemberRow/RoleRow + add/remove/list × 3（9 本）を追加。**test 472→473**・全ゲート緑・新規依存なし。ルート 114→117/152。**残 botAttributeRoutes**: usage/modules/assistant-{config,gemini-key〔crypto〕,guild-options〔Discord live〕}。
- 追記（2026-07-14q）: **Bot 単位ペルソナ設定 Web-API（assistant/persona 1 本）実装**（`bot_attribute_routes` 拡張）。commit `2a51645`。POST `/api/bots/assistant/persona`（requireOwnedBot + personaId 検証〔解除/整数/owner or 公開のみ 403〕 + 監査）。bot_repo に get_persona_owner_public/set_bot_persona 追加。**test 471→472**・全ゲート緑・新規依存なし。ルート 113→114/152。**残 botAttributeRoutes**: usage/modules/assistant-{gemini-key〔crypto〕,guilds,members,roles,guild-options〔Discord live〕,config〔集約 GET〕}。
- 追記（2026-07-14p）: **Bot ギルド共有ノート Web-API（guild-note 2 本）実装**（`bot_attribute_routes` 拡張）。commit `1854dfd`。GET/POST `/api/bots/assistant/guild-note`（requireOwnedBot + guildId 検証 + 長さ 10000〔UTF-16〕）。bot_repo に set_bot_guild_note 追加 + require_owned_bot/is_snowflake/format_commas ヘルパ。**test 469→471**・全ゲート緑・新規依存なし。ルート 111→113/152。**残 botAttributeRoutes**: usage/modules/assistant-{gemini-key,persona,guilds,members,roles,guild-options}。
- 追記（2026-07-14o）: **外部 Webhook Web-API（webhookRoutes 6 本）実装**（新クレート `yuuka-webhook`）。commit `d29a1ad`。POST `/hook/{token}`（レート制限 429→404→410→200 + **WebhookProcessor シーム** fire-and-forget）+ `/api/webhooks`(GET)/create/update/delete/deliveries。repo（create/list/get/update/delete/deliveries・generate_token・has_secret ビュー）+ シークレット SystemCrypto 暗号化。build_app に webhook_routes 貫通（crypto 注入）。WebConfig.base_url 追加。**test 467→469**・全ゲート緑・新規依存なし。ルート 105→111/152。**残 webhookRoutes**: /hook の実処理（HMAC/通知/todo/reminder＝WebhookProcessor 実装）。
- 追記（2026-07-14n）: **Bot 属性 Web-API（botAttributeRoutes プリセット 4 本）実装**（`yuuka-orchestrator::bot_attribute_routes`）。commit `54b14b5`。GET `/api/bots/presets`・POST `/api/bots/attributes`（requireOwnedBot + applyBotPreset + 監査）・GET/POST `/api/admin/bot-attribute-settings`（admin・表示名 + レート制限既定値）。preset モジュール消費 + レート制限 system_settings（getRateLimitSettings パリティ）。**test 465→467**・全ゲート緑・新規依存なし。ルート 101→105/152。**残 botAttributeRoutes**: usage/modules/assistant-*（暗号・ギルド設定サブシステム）。
- 追記（2026-07-14m）: **Bot 管理 Web-API（botRoutes CRUD 4 本）実装**（`yuuka-orchestrator::bot_management_routes`）。commit `6d4bf79`。GET/POST/DELETE `/api/bots` + POST `/api/bots/profile`。toBotView（botViewSchema ホワイトリスト・機密非出力）+ botHealth（**BotViewRuntime シーム**・既定 Null=非稼働）+ resolveBotApplicationId（token_enc を `SystemCrypto::decrypt_text` 復号→base64url app-id・crypto は routes_with 注入）+ 作成時 applyBotPreset/監査。**test 463→465**・全ゲート緑・新規依存なし。ルート 97→101/152。**残 botRoutes**: POST /api/bots/sync-discord（Discord live 依存・別途）。
- 追記（2026-07-14l）: **bot-CRUD リポジトリ層移植**（`bot_repo`・Node `botRepo` の Bot インスタンス CRUD）。commit `06c53f6`。BotDetail（Web ビュー用列 + app-id 導出用 token_enc〔非公開〕）+ get_bot_detail/list_bots_for_user（owner+system_default+共有 active JOIN）/create_bot/delete_bot/update_bot_profile。**bot-CRUD Web-API の DB 基盤**。**test 461→463**・全ゲート緑・新規依存なし。**残 bot-CRUD Web-API**: GET/POST/DELETE /api/bots + profile の routes + toBotView（BotDetail + preset 表示名 + **BotRuntime health seam**〔running/connected/shared〕+ resolveBotApplicationId〔token 復号→base64url〕）。sync-discord は Discord live 依存で後回し。
- 追記（2026-07-14k）: **Bot プリセット/ケーパビリティ解決サービス移植**（`yuuka-orchestrator::preset`・Node `services/botCapabilities.ts`）。commit `feac86c`。BotPresetId（Secretary/McpAssistant）+ capabilities/表示名（system_settings 上書き）+ preset_id_for_capabilities + apply_bot_preset + list_presets。**bot-CRUD（GET/POST/DELETE /api/bots）の基盤**（toBotView の preset 解決 + 作成時 applyBotPreset が消費）。**test 457→461**・全ゲート緑・新規依存なし。**残 bot-CRUD**: toBotView（preset 表示名 + BotRuntime health seam〔running/connected/shared〕+ resolveBotApplicationId〔token 復号→base64url〕）・list_bots_for_user（全列）・create_bot/delete_bot/update_bot_profile・sync-discord（Discord live）。
- 追記（2026-07-14j）: **Bot 共有 Web-API（botRoutes shares 3 本）実装**（`yuuka-orchestrator::bot_share_routes`）。commit `c860a83`。`GET /api/bots/shares`（閲覧・作成者のみ・shared_username 付与）・`POST /api/bots/shares/invite`（招待・自己招待 400・未登録 404・公開ペルソナ名 DM 添付・ShareInviteDm シーム）・`POST /api/bots/shares/revoke`（作成者 or Admin・越権時監査）。bot_repo に BotShareRow/create_share_invite/list_shares_for_bot/get_username 追加 + revoke_share を bool 返しへ。**test 455→457**・全ゲート緑・新規外部依存なし。ルート 94→97/152。**残（botRoutes）**: GET/POST/DELETE /api/bots・sync-discord・profile（Discord ランタイム health 依存・別増分）。
- 追記（2026-07-14i）: **デスクトップ配布 Web-API（desktopClientRoutes 2 本）実装**（supervisor `desktop_dist`）。commit `b9660b7`。`GET /api/desktop/info`（配布バイナリのメタ・未配置は available:false）・`GET /api/desktop/download`（exe 添付配信・未配置 404）。配布 dir は env `DESKTOP_DOWNLOAD_DIR`→既定 `cwd/dist/downloads`。exe 非同梱ランタイムでは available:false/404 だが route surface を完全移植。**test 453→455**・全ゲート緑・新規外部依存なし。ルート 92→94/152。
- 追記（2026-07-14h）: **利用申請 Web-API（memberRequestRoutes 4 本）実装**（`yuuka-orchestrator::member_request_routes`）。commit `c0f5b20`。`POST /api/bots/member-requests`（申請）・`GET .../mine`（自分の申請）・`GET /api/bots/member-requests`（オーナー一覧・status/botId 絞り・Admin は任意 Bot）・`POST .../{id}/decide`（承認/却下・承認時 bot_members 追加）。既存 bot_repo::submit/decide_member_request を再利用し SubmitResult/DecideResult に code を追加して HTTP status 分岐（Node parity）。member-request 一覧 3 本 + list_bots_owned_by を追加。DM は MemberDmSender シーム（既定 NullMemberDmSender）。**test 449→453**・全ゲート緑・新規外部依存なし（orchestrator へ axum/serde/yuuka-auth を追加）。ルート 88→92/152。
- 対象ブランチ: `feature/rust-rewrite`（未 push・HEAD=`b77506d` の上に未コミット差分）
- git HEAD: `b77506d`（P2 ドメイン Web-API 拡張）+ 本セッションの縮退シーム解消差分（未コミット）
- 前提資料: [review-2026-07-06-fix-policy.md](review-2026-07-06-fix-policy.md)（修正方針の唯一の基準）・[review-2026-07-09-batch4-6.md](review-2026-07-09-batch4-6.md)・[PLAN.md](PLAN.md) §11（移行ロードマップ）
- **本セッション（2026-07-09e）の成果**: **P1-3 Discord live 化**を実装 — twilight 転送層（既存・不活性）を**起動配線**した。(1) 注入ポートの**本番 DB 実装**を新設（`yuuka-orchestrator`）: `DbBotDirectory`（Node `botRepo`/`botAttributesRepo`/`userRepo` パリティ・Bot メタ/list/共有アクセス/トークン復号[`SystemCrypto`]/メンバー・許可ロール・登録判定）・`DbMembership`（申請 submit/decide=承認で `bot_members` 追加・共有 accept/revoke・公開ペルソナ import・全て writer actor 単一 Tx）・`InMemoryRateLimiter`（Node `botRateLimit` 固定窓 5/分・100/日・1000/ギルド日・`system_settings` 上書き）。(2) **汎用モード**（`ChatEngine::process_guild`/`process_bot_dm`）実装 — Bot 専用 Gemini キー（`getBotGenAI` パリティ・`BOT_DEFAULT_MODEL`）・ギルド/DM 分離コンテキスト（`[名前]:` プレフィックス・guild 30 件/DM 15 件）・Bot 単位ペルソナ + 共有/個人ノートの `buildGuildSystemInstruction` 移植・FC ループ。(3) **`main.rs` 配線** — `DiscordManager::new(ports + processor=ChatEngine)` → `prepare()` で共有 `Messenger` 生成、各 `TenantRunner` を `DiscordTenantService` で **Supervisor 監督下**（panic 隔離 + 指数バックオフ・恒久クローズは非再起動）へ。ゲートウェイ起動は **`YUUKA_RUST_DISCORD` env ゲート**（既定 off＝二重 gateway/二重応答の回避・`YUUKA_RUST_CRON` と同思想）。**P1-4**: cron の `NullNotifier` を `Messenger`（`impl services::Notifier`）へ差し替え＝リマインド等が実 Discord へ配信可能に。**P1-1 残**: 登録コード DM を `MessengerRegistrationDm`（合成ルートアダプタ）で `Messenger` 経由に配線＝`/api/register` が実際に DM を送る。機械ゲート全緑（build/release/clippy-D/deny/**test 303**・+15: ポート DB 実装 7・汎用モード 3・guild prompt 2・build_contents 回帰 3）。
- **本セッション（2026-07-10）の成果**: **P1-3 未コミット diff の parity レビュー**（7 次元並列 + 各指摘を敵対的検証 = confirmed 14）を通し、確定 6 件を修正した。**(H) build_contents 二重ユーザーターン** — persist-before-load で履歴末尾に既にある発言を `build_contents` が再追加していた（Node `buildContentsFromHistory` は空履歴のときだけ `message.text` を積む）。修正: 空履歴のみ lone user turn・非空は添付のみ末尾 user content へ合流。**(H) notifier の DM フォールバック欠落** — `DiscordMessenger::send_to_user` が Channel 解決失敗で即 `false`（cron 経路は deliver_final を通らないため fallback 不能＝リマインダー永久リトライ）。修正: Channel 解決不能時に DM へフォールバック（Node `sendToUser` notifier.ts:130-142）。**(M) owner DM の context floor** — DM が秘書 floor を流用していた。修正: `recent_bot_dm_context`（floor `context_floor:{botId}:dm:{userId}`・Node `getBotDmContext`）を新設し DM 分岐で使用。**(M) レート制限の日窓** — `:d` 固定キー + 25h TTL の転がり窓だった。修正: `todaySuffix`（ローカル暦日 `YYYYMMDD`）を日キーへ付与し暦日境界でリセット。**(L) describe_incoming の trim**（空白のみを添付プレースホルダへ）・**(L) レート上限設定の parseInt 寛容パース**（先頭数字のみ解釈）。**defer（doc 化済み）**: 利用申請/決定の owner・applicant DM 未送（明示的な縮退シーム＝`DbMembership` への messenger 注入待ち・§P2-A）／`has_gemini_key` は presence 判定で Node `getBotGenAI` の復号検証より弱い（低・むしろ復号エラーを表面化＝ops 良）／汎用モードの LLM エラー文言（rate-limit/server-error 別の ⚠️ ＝`guildErrorResult` 相当）は未分類（低・`TurnError` へ分類貫通が必要）。
- **本セッション（2026-07-10b）の成果**: **P1-3 の残 3 件（縮退シーム）を実装**。**(1) 能力ゲート（P2-B）**: `ToolContext` に `mode: TurnMode`（Secretary/GuildAssistant）追加・`Tool::exposure()`（既定メソッド＝現行 23 ツールは全て secretary 分類）・`ToolExposure::is_visible`（経路 × 能力）・`NativeProvider::list` で filter・engine が `bot_repo::parse_capabilities`（Node `parseCapabilities` パリティ＝null/空/非配列/失敗は秘書相当フル）で `ctx.capabilities` を注入。秘書経路は `caps.has("secretary")` で 23 ツール露出、汎用モードは secretary ツールを一切露出しない（Node `getGuildAssistantFunctionModules` は guild-assistant モジュールのみ・未移植）。**残**: ユーザー別 `enabledModules`（`bot_user_modules`/`bots.enabled_modules` の selectable 絞り込み・Node 第 2 次元）は未移植＝module 選択 UI 系（P2-A・native.rs にコメント明記）。**(2) member-request DM**: `SubmitOutcome`/`DecisionOutcome` に owner/applicant/bot_name/request_id を露出し、interaction ハンドラが既存 text-exact ヘルパ（`send_member_request_dm`/`send_member_decision_dm`）を DB 確定後に呼ぶ（`MemberDmSender` ポートを `InteractionDeps` へ注入・fire-and-forget）。submit→owner 受付 DM（承認/却下ボタン）・decide→applicant 結果 DM。**(3) 汎用モード LLM エラー文言分類**: `classify_gemini_error`（429=`RateLimited`・{500,502,503,504}=`ServerError`・他は None→generic）で秘書/汎用の別文言（秘書は「（トークン枯渇など）」「（503等）」付き）を `Ok(TurnReply::text)` で返す（履歴非保存・Node `guildErrorResult`/`processMessage` catch）。**parity レビュー**（4 次元 + 敵対的検証 confirmed 7・全て low/medium・high 無し）を通し 7 件対応（capabilities NULL 耐性・stale doc・comment 正確化・classify/label_or 回帰テスト追加等）。機械ゲート全緑（build/release/clippy-D/deny/**test 310**・+7）。
- **本セッション（2026-07-10c）の成果**: **フォールバック関連の横断精査**（Discord 配信/会話エンジン/インフラの 3 クラスタ・約 25 箇所を Node と突き合わせ）。**修正 2 件**: (H) `NO_KEY_MESSAGE`（⚠️ Gemini キー未設定）を Rust だけが `message_logs` に assistant 保存していた — Node `processMessage` の catch は `saveAssistant` を通らず返すだけ＝⚠️ 定型応答は履歴非保存が正（保存すると以後の文脈ウィンドウを警告文で汚染）。除去 + 非保存を回帰 assert で固定。(M) `/ws/chat` の添付上限が Rust ハードコード 20MB — config `DESKTOP_MAX_UPLOAD_MB`（Node `desktopMaxUploadMb`）を core `Config` に追加し `ws_routes` へ貫通、too_large 文言も Node の上限値入り文面に一致。**精査で反証**: settings 空白のみ値の扱い（両者 `" "` 採用＝一致）・context floor パース失敗→0（一致）・レート上限 parseInt（一致）・deliver_final の DM 再送は全文再送で二重配信なし（一致）・SPA fallback は API を食わない（一致）。**意図的 divergence として明文化**: resolve_client/send_owner_dm の readiness 非ゲート（twilight REST は gateway 非依存＝Node が拒否するケースでも配信できる・改善側）／`or_default` の安全側 deny（Node は DB 例外→エラー返信・WAL 読取は BUSY 稀のため頑健性優先）／セッション 502（M-1 レビュー済み契約・Node は 401→再ログイン自己回復）／config fail-fast（Node は NaN 黙殺・Rust は起動拒否＝安全側）。**既知の未移植**: Node `fallbackText` の browser 分岐（browser ツール未移植のため到達不能・P2-B と同時に移植）。機械ゲート全緑（**test 310**・clippy-D/deny exit 0）。
- **本セッション（2026-07-10d）の成果**: **ペルソナ入りエラー応答**（ユーザー要望による Node からの意図的拡張）。ターン失敗を内部で `TurnFailure::{Llm, NonLlm}` に分類し、**非 LLM エラー（DB 障害等＝LLM は生きている）では固定の GENERIC_ERROR をやめ、LLM にペルソナ口調のエラー報告（1〜3 文・技術用語なし）を生成させて返す**。秘書経路＝ユーザーキー + アクティブペルソナ（無ければ `DEFAULT_PERSONA`）、汎用モード＝Bot 専用キー + Bot ペルソナ。生成はツール無し単発・履歴非保存（⚠️ 定型と同じ扱い）。**フォールバック連鎖**: ペルソナ生成→（生成不能: キー無し/復号不能/生成失敗）→従来の固定文。**LLM 関連エラー（レート/サーバー/鍵復号不能/backend 構築失敗/上流未分類）は従来どおり固定文**（LLM を呼べない/信頼できないため）。回帰テスト 3 件（秘書ペルソナ応答・キー無しフォールバック・汎用モード Bot ペルソナ応答）。機械ゲート全緑（**test 313**・clippy-D/deny exit 0）。
- **本セッション（2026-07-12）の成果**: **Web/LLM-API 面の本番投入準備**。cargo 全ゲート緑（build/clippy -D/deny exit0/**test 330**・+17）+ 実バイナリのライブ疎通（N2/B5/B6/CSRF）を確認したうえで、2026-07-12 監査の確定ブロッカー4件と、Web API 面の**敵対的セキュリティレビュー**（5次元×各指摘を独立検証＝29エージェント/23指摘/確定15・判定=**canary go**）の確定 low 指摘を修正した。**確定ブロッカー4件**: (B5) `/ws/chat` の CSWSH 退行＝汎用 `AuthenticatedUser`（Cookie 優先）が WS upgrade の ambient Cookie を受理していた → **Bearer 専用 `BearerUser` extractor** を新設し Bearer 限定化（Node `getBearerUser` パリティ・構造的 CSWSH 不能・回帰テスト追加）。(B6) 未登録 `/api/*` GET が SPA index.html(200) で握り潰し → `static_files.rs` に **全メソッド `/api/*`→JSON 404**（`{success:false,message:"APIエンドポイントが見つかりません。"}`・Node `server.ts:339`）。(B4) reminder `trigger_at` の字句比較バグ＝`T` 区切り ISO を生保存 → cron の空白区切り `datetime('now','localtime')` と字句比較で当日発火が翌日まで遅延 → `datetime::to_db_datetime`（Node `toDbDateTime` パリティ・`Z`/オフセット/小数秒/日付のみ対応）を新設し repo 境界正規化 + tool/**web route 双方で入力検証**。(N2) 暗号鍵未設定でも起動継続（Rust 固有退行）→ main.rs に `require_encryption_secret`（未設定/32 文字未満は起動拒否・Node `index.ts` §6.2）。**セキュリティレビュー確定 low 指摘**: (#1) CSRF 信頼アンカーがクライアント供給 `Host` だった → **`config.base_url` 由来の `allowed_host` allowlist** へ（Node `isAllowedHost`・Host 注入耐性・`WebConfig` へ貫通）。(#2) `Sec-Fetch-Site` が `cross-site` 以外を無検証許可 → `cross-site` のみ即拒否・他値は Origin allowlist へ委譲（`same-site` 兄弟サブドメイン対策）。(#3) 空 `Bearer ` が CSRF 免除される潜在バイパス → `has_bearer` を非空トークン必須に。(#4) `ScopedJson` の serde エラー文言（型/フィールド名）露出 → 固定文言化。**確定したが本セッション未対応（低・own-user/fail-closed・doc 済）**: (#5) reminder 過去日時の拒否/繰り返し自動前進の未移植（cron next は上位 crate 依存・P3-3 M-9 の残）／(#7) 不完全添付でフレーム全体を internal error（fail-closed の意図的厳格）。レビューが**反証/refuted した非問題**（対応不要）: WS Bearer 専用化の完全性・botId IDOR 防止・WS エラー/秘密の非漏洩・接続タスク panic 隔離・CSP/セキュリティヘッダ・error→status の内部 Display 秘匿・9 ドメインの認可/multi-tenant スコープ厳密性。**判定: 経路A（strangler カナリア・単一 vhost HTTPS）= GO**（critical/high/medium ゼロ）。経路B no-go は不変（routes 22%/tools 27%）。
- **本セッション（2026-07-14d）の成果**: **設定系 Web-API（P2-A settings）を新規 `yuuka-settings` クレートで実装**（Node `settingsRoutes.ts` パリティ・**6 エンドポイント**）。**test 412→422**・全緑 / clippy -D 緑 / deny 緑（**新規依存なし**）。**実装**: `POST /api/settings/{profile,password,delete-account,user,gemini,backup}`。**構成**: `AdminRuntime` と同じ `Extension(SettingsRuntime)` 方式 + `AuthenticatedUser` extractor で auth:"user" 型強制 + `build_app` に `settings_routes` を貫通（merge 順 = `framework→auth→admin→settings→ws→domains`）。所有 Bot 停止は admin と**同一の `BotRuntime` シーム**（`NullBotRuntime`）を共有。**セッション再発行**: profile=現セッション失効→新規発行→Set-Cookie、password=**全セッション失効 + デスクトップトークン全失効 + 監査 + 新規発行**（Node `setSessionCookie` パリティ）。**yuuka-auth を拡張**（設定系で再利用するため公開）: `hash_password`（bcrypt cost12）pub 化・`build_session_cookie` pub 化・`SessionCookieToken` extractor 追加・`revoke_all_for_user`（デスクトップトークン全削除）新設。gemini はキー形式検証（`^AIza[0-9A-Za-z_-]{30,}$`・regex 非依存）+ 暗号化 or マスク時 keep-current・user は Node `key in body` 部分更新意味論（present 列のみ SET・`notify_id` は `Some(None)` で明示 NULL）・backup は `extractDriveFolderId`（ID/`/folders/`/`?id=` URL）+ interval `[1,720]`/generations `max(1)` clamp。**敵対的パリティレビュー**（6 エンドポイント × 検証/文言/status/セッション再発行/副作用順/SQL）で **HIGH 0**。**意図的 divergence（accepted・対応不要）**: (1)**型付き `Json<T>` 抽出 = 空/型不一致/非 JSON ボディは axum の 4xx**（Node は `typeof` coercion + `{}` 既定で握り潰し）＝**auth/admin クレートと共通の既存規約**（実フロントは常に正 JSON 送信）／(2)username UNIQUE 衝突は 400「既に使われている」（Node は偶発 500）＝安全側／(3)`isLikelyGeminiKey` 末尾 `\n` は trim 後で到達不能／(4)role 大小文字 `eq_ignore_ascii_case`（DB は小文字のみ・到達不能）／(5)`Number()` の hex/配列 coercion 差（フロントは数値送信）。**deferred（Google Drive/Calendar HTTP サブシステム依存・別増分）**: `GET /api/status`（多ドメイン集計 + カレンダーキャッシュ）・`GET/POST /api/settings/discord`（Bot トークン設定 + 再起動フロー）・`google/oauth/{url,callback}`・`calendars`・`backup/trigger`。
- **本セッション（2026-07-14c）の成果**: **管理系 Web-API（P2-A admin）を新規 `yuuka-admin` クレートで実装完了**（Node `adminRoutes.ts` パリティ・15 エンドポイント）。**test 402→412**・全緑 / clippy -D 緑 / deny 緑（**新規依存なし**＝既存 workspace dep のみ）。**構成**: 認証発行ルータ（`yuuka-auth`）と同じ `Extension(AdminRuntime)` 方式（`AppState` に入れると web→auth 循環になるため分離）+ `AdminUser` extractor で auth:"admin" を型強制 + `build_app` に `admin_routes` を貫通（`framework→auth→admin→ws→domains` の merge 順）。**実装エンドポイント**: default-bot/token（トークン暗号化保存 + upsert）・stats（6 集計）・system-settings GET/POST・users・users/role（自己降格ガード + セッション一括失効）・users/delete（所有 Bot 停止 + セッション失効 + 404 分岐）・audit-logs（前方一致 + ページング）・bots（オーナー名 + hasCustomToken + isRunning）・bots/suspend/unsuspend・invite-codes GET/POST/revoke/delete。レスポンス JSON は Node `sendJson` とバイト単位一致（フラット `{success, ...}`・camelCase/snake_case をフィールド別に再現・日本語メッセージ verbatim）。**新規**: `SessionStore::destroy_all_for_user`（Node `destroyAllSessionsForUser` パリティ・Redis + in-memory の `user_sessions:{id}` セット一括失効・ロール変更/削除の即時反映）。**Bot runtime シーム**: `restartDefaultBot`/`stopCustomBot`/`customClients.has`→`isRunning` は `BotRuntime` ポート越しに委譲し、Discord gateway 未配線（`YUUKA_RUST_DISCORD` 既定 off）のため既定 `NullBotRuntime`（no-op・非稼働扱い）へ縮退（**DB 効果＝トークン暗号化保存・suspend フラグ・ユーザー削除は常に完全に働く**・gateway 配線時に実 `BotRuntime` を注入すれば live 化）。**敵対的パリティレビュー**（14 エンドポイント × method/path/検証/エラー文言/レスポンス形/status/副作用順序/SQL 意味論）で確定 **0 バグ**。**意図的 divergence として明文化（レビュー確定・全て安全側または到達不能・対応不要）**: (1) system-settings の URL 検証は絶対 URL（`scheme://host`）または `/` 相対のみ許可＝Node `new URL()` が通す opaque scheme（`mailto:`/`data:`/`javascript:`）を拒否（**安全側**＝policy リンクの XSS scheme を遮断・正当な https/相対は不変）／(2) audit-logs の `limit`/`offset` 非数値は寛容パースで既定値へ（Node は `NaN`→SQLite bind で 500・Rust は **堅牢側**）／(3) 監査ログ書込失敗は best-effort（Node は commit 後に throw→500・`add_audit_log` の既存方針＝本処理を落とさない）／(4) system-settings GET の DB 読取失敗は 500（Node は try/catch で config 既定へ縮退・**fail-closed の既存方針**）／(5) `owner_username` 空文字は `COALESCE`（NULL のみ '不明'）＝Node `|| '不明'` と僅差だが username は NOT NULL + 登録時非空拒否で到達不能。
- **本セッション（2026-07-14b）の成果**: **縮退シーム（後退機能）5 件の実装完了**（監査ワークフローで「動くが機能しない silent no-op」を棚卸し → self_contained/feasible の 5 件を完全実装 → 敵対的レビュー[4 次元 20 エージェント・confirmed 10/refuted 6]で確定した medium 2 件を修正。**test 386→402**・全緑 / clippy -D 緑 / deny 緑・新規 crate 追加なし[既存 workspace dep の base64/tokio/getrandom を timeline へ新規参照]）。**(1) finance 月次集計**: `GET /api/expenses` に total/incomeTotal/breakdown/trend を追加（Node `getMonthlyTotal`/`getMonthlyCategoryBreakdown`/`getMonthlyTrend` パリティ・trend は当月含む過去 6 ヶ月をロールオーバー整数計算 + 欠損月ゼロ埋め）。**これまで HUD が黙って 0/空表示になっていた真の silent 後退を解消**。**(2) timeline cross-domain 副作用**: `type=expense` は `expenses` へ二次登録し `expense_id`/`expense_category` を連結（単一 writer tx・finance settle_plan と同パターン・`expenses.amount` は INTEGER 列のため round して i64 格納＝**medium 修正**: 非整数だと finance list/get が i64 デコードで 500 に落ちる退行を防止）、`type=task_done` は `todo_id` 指定時に紐付き todos を `done` 更新（Node `completeTodo`）。route/tool 双方で対応。**保存は成功するが expenses 二重登録/todo 完了が黙って落ちる silent 後退を解消**。**(3) personal clipboard addEntry**: `addClipboardEntry`/`listClipboardEntries`/`deleteClipboardEntry` ツール + `ClipboardRepo::add`（TTL 付き・0=無期限）を追加（Node `clipboardFunctions.ts` パリティ）。**deleteExpired cron は既に実装済み**（`ClipboardCleanupService`・doc 誤記を訂正）。**(4) playbook スケジューラ/実行エンジン**: `PlaybookRunner` ポート（yuuka-services 新設 `turn.rs`）+ `ServiceContext.playbook_runner` + 実 `PlaybookScheduleService`（EveryMinute tick・cron due 判定 = `next_after(expr, last_run_at||created_at) <= now`・冪等）+ cross-user scan/run 記録（yuuka-playbook 新設 `cron.rs`）+ `PlaybookRunnerAdapter`（main.rs で `ChatEngine` へ橋渡し・循環回避）。deferred no-op を実体へ差し替え。**設定を保存してもマクロが自動実行されない silent 後退を解消**（`YUUKA_RUST_CRON=1` 時）。**(5) timeline media**: `POST /api/timeline/media`（base64 アップロード）+ `GET /api/timeline/media/{filename}`（認証付き配信・path traversal 対策）+ `media.rs`（MIME 検証・ファイル名 = CSPRNG suffix＝**medium 修正**: 単調カウンタは推測可能で所有者非照合配信の唯一の障壁を弱めるため getrandom へ）。`WebConfig.media_dir`（既定 `data/media`）追加。tool 経由の Discord 添付 URL 取得のみ reqwest 依存で deferred。**意図的 divergence として明文化（レビュー確定 low・対応不要）**: finance/plan の category 空白のみ拒否（Rust は Node より厳格・安全側）／finance `/api/expenses/add` は float amount を 400 拒否（i64 DTO・型安全・timeline expense 経路は round で受理）／`truncate_chars` は非 BMP で JS UTF-16 と僅差／clipboard `ttl_hours` の null・content 非文字列の coercion 差（Gemini は常に文字列 content・Rust は安全側）／playbook tick は復帰直後に取りこぼしを最大 1 回 catch-up 発火（Node は catch-up 無し・有界）。
- **本セッション（2026-07-13）の成果**: **P2 ドメイン Web-API 拡張**（`b77506d`・5 ドメインクレートに Node parity の新規 HTTP エンドポイント + DB 層 + DTO + テストを 21 エンドポイント追加。**test 330→386**・全緑 / clippy -D 緑 / build 緑・新規依存なし＝deny 緑不変）。全リポメソッドは `WHERE user_id=? AND bot_id=?` でスコープ束縛・全クエリ `params!` でパラメータ化・DTO は `user_id`/`bot_id` 非露出のクリーンビュー。**(finance)** 予算上限（`/api/expenses/budget-limits` GET/POST・`/delete`）+ 支払い予定の消込（`/plans` GET/`/add`/`/pay`/`/delete`）。`settle_plan` は Expense 記録・予定 settled・紐付き ToDo 自動 done を単一 Tx で実行。**(personal)** コンテキストノート（`/api/context-note` GET/POST・upsert）+ クリップボード（`/api/clipboard` GET・`/delete`・期限切れ自動除外）。**(playbook)** スケジュール（`/api/playbooks/schedules` GET/`/save`/`/toggle`/`/delete`）+ 実行履歴（`/runs` GET）。**(timeline)** 計画ブロック CRUD（`/api/timeline/plan` add/`/update`/`/delete`・day に blocks 同梱）。`UpdatePlanBlock` は `Option<Option<T>>` で Node の `"key" in obj` 意味論を再現。**(todo)** gantt/someday/detail GET + update/progress POST。子孫は再帰 CTE でスコープ束縛収集。`PriorityUpdate` は 3 値（据置/クリア/設定）で Node parity。**意図的な縮退シーム（本セッションで新設・下記 P2-A/C に反映済み）**: (1) **playbook スケジューラは未起動** — schedules/runs は永続化されるが、cron 式検証（croner がクレート依存に無い）と実行エンジン（`executePlaybook`）は deferred。**設定を保存してもマクロは自動実行されない**（reminder と同方針・別タスク）。(2) **finance** — receipt OCR（`upload-receipt`）と月次集計（total/breakdown/trend）は deferred。(3) **timeline** — media 保存/配信・`type=expense`/`type=task_done` の cross-domain 副作用は deferred。(4) **personal** — クリップボードの追加（`addEntry`）と TTL 一括削除 cron（`deleteExpired`）は deferred（誕生日リマインド cron は移植済み）。
- **前セッション（2026-07-09d）の成果**: **P1-1 資格情報認証発行**（`SessionStore` 発行+検証・7 ルート・bcrypt `$2b$` cost12・招待/監査/レート制限）+ **P1-2 会話**（`yuuka-orchestrator`・`/ws/chat`）。
- **前々セッション（2026-07-09c）の成果**: P0-1〜4・P1-5（暗号層）・P1-6/7・P1-4 アダプタ・P3-4/5。

---

## 0. 現状サマリ（判定）

**「ほぼ本番（Node 完全置き換え）」としてはまだ使えない。** 基盤設計は堅牢でユニットは全緑だが、ユーザーが実際に触れる経路（ログイン・会話・Discord・通知・秘密情報復号）が揃っていない。

| 面 | 実測 |
|---|---|
| `cargo build --release --workspace` | ✅ exit 0 |
| `cargo clippy --workspace --all-targets` | ✅ exit 0（ts-rs 良性 warning のみ） |
| `cargo test --workspace` | ✅ **501 passed / 0 failed**（2026-07-15 認証情報アクセス制御 + 利用量サマリ + デバイスフロー + ペルソナ marketplace で累積） |
| `cargo deny check` | ✅ **exit 0**（新規依存なし＝既存クレートのみで実装） |
| 保存時暗号層（Argon2id/AES-256-GCM） | ✅ **実装済**（P1-5・Node ゴールデンベクタでバイト単位パリティ・鍵ローテ起動時配線） |
| 認証発行（login/setup/logout/register/users） | ✅ **実装済**（P1-1・セッション発行 + bcrypt + 招待 + 監査 + レート制限。**登録 DM は P1-3 で開通**・OAuth は残） |
| HTTP ルート被覆 | 80 / 152 パス ≒ **53%**（認証 7 + `/api/me` + 9 ドメイン CRUD + 管理系 15 + **設定系 6**〔2026-07-14 settings〕） |
| Gemini ツール被覆 | 71 / 87 native ≒ **82%**（動的 MCP 0。2026-07-14 に conversation 1 + **richContent(showRichContent) 1**〔Embed 配管新設〕含む多数を追加。残 16 は browser 系 10〔chromium 非同梱で意図的 defer〕/ sendChart〔要チャート描画〕/ runBriefingNow〔weather/RSS HTTP〕/ organize・applyTaskPriorities・getRecentActionHistory〔Node 側で範囲外〕/ MCP 動的） |
| チャットオーケストレーション（秘書ターン） | ✅ **実装済**（P1-2・`yuuka-orchestrator`・実 TurnProcessor・統合テスト緑） |
| 汎用モード（guild/owner DM ターン） | ✅ **実装済**（P1-3・`process_guild`/`process_bot_dm`・Bot 専用キー + ギルド/DM 分離文脈・統合テスト緑。能力ゲート=全ツール露出は P2-B） |
| WebSocket `/ws/chat`（デスクトップ会話） | ✅ **実装済**（P1-2・Bearer 認証 + ready/status/done・live 統合テスト緑。interaction/deferred は縮退） |
| Discord live（gateway 起動 + Supervisor 監督） | ✅ **配線済**（P1-3・DB ポート + `DiscordManager.prepare` + `DiscordTenantService`。既定 off・`YUUKA_RUST_DISCORD=1` で起動＝Node bot 停止後） |
| Gemini FC ループ本体 | 1:1 移植 ≒ 95% 完成（秘書 + 汎用モードの両経路から到達可能・P1-2/P1-3） |
| 通知配信ブリッジ | ✅ **配線済**（P1-4・main の `NullNotifier`→`Messenger` 差し替え。デフォルト Bot 起動でリマインド等が実配信） |
| 登録 DM ブリッジ | ✅ **配線済**（P1-3・`MessengerRegistrationDm`＝`RegistrationDm`↔`Messenger` の合成アダプタ。`/api/register` が実 DM 送信） |

**進め方の2経路:**
- **経路 A（strangler 並走カナリア）** — Node が認証/会話/Discord/暗号を担い、Rust は移行済み CRUD の一部だけを共有 Redis セッション前提で配信。§7-A の前提を満たせば数日規模で到達可能。**（P1-1 により Rust 単独でのセッション発行も可能になった＝Rust だけでログイン→CRUD が回る。）**
- **経路 B（単独ほぼ本番）** — Rust だけで完結。P1-1〜P1-7 は全て着地（残は P1-1 の OAuth のみ＝P2-A へ移送）。残は主に P2（機能パリティ）+ P3（CI/fmt/残指摘）。**Discord ライブ確認**（実トークンでのメンション/DM 応答・二重処理回避のカットオーバー）は要実機検証。

---

## 優先度の凡例

- **P0 — データ安全 / 起動前必須**: これを飛ばすとデータ喪失・即クラッシュ。何より先。
- **P1 — 致命ブロッカー**: 単独「ほぼ本番」を名乗るのに不可欠。無いとユーザーが何もできない。
- **P2 — 機能パリティ**: Node にあって Rust に無い機能面。順次埋める。
- **P3 — 品質 / 運用衛生**: CI・fmt・古い記述・LOW 指摘など。

チェックボックスはそのまま進捗トラッキングに使用可。

---

## P0 — データ安全 / 起動前必須（最優先）

- [x] **P0-1 未コミットの V17 マイグレーション修正をコミットし、以後 V17 を凍結する** — 済（`61934e8`）
  - 内容: working tree にある `crates/yuuka-db/migrations/V17__baseline.sql`（fts5 仮想テーブル + トリガ ×3 に `IF NOT EXISTS`、末尾に `system_settings.schema_version='17'` upsert）と `crates/yuuka-db/src/schema.rs`（refinery の SQLITE_BUSY/LOCKED を Transient 分類）を確定コミットする。
  - なぜ: 現 HEAD の V17 のまま既存 DB へ起動すると「object already exists」で**移行 Fatal**。かつ `schema_version='17'` スタンプが無い DB を後で Node が開くと legacy-v1 誤検出で**コアテーブル全 DROP（全データ喪失）**。
  - 完了条件: コミット済み。カットオーバー後は **V17 を二度と編集せず、変更は V18+** で行う旨を運用ルールとして明記（[review-2026-07-06-fix-policy.md](review-2026-07-06-fix-policy.md) §157）。
  - 注意: 旧 Rust バイナリで一度でも migrate した dev/test DB があると `refinery_schema_history` に旧 checksum が残り `abort_divergent` で Fatal → その DB は `refinery_schema_history` 削除か作り直し。現行 `data/yuuka.db` は履歴なしなので修正込みなら初回起動クリーン。

- [x] **P0-2 cron の所有を片側専任にする（二重 writer ハザード回避）** — 済（`deploy/README.md` 「移行期の運用注意」に cron 所有ルール明記。コンテナ Rust=cron 所有／旧 systemd 同時起動禁止／経路 A は `YUUKA_RUST_CRON=0`）
  - 内容: `YUUKA_RUST_CRON=1` が `Dockerfile:116` に焼き込まれている。Node cron と同時稼働させない。Rust cron を使うなら Node 側 cron を停止、使わないなら Rust の env を落とす。
  - なぜ: 同一 SQLite WAL への writer 同時 2 つは即 `SQLITE_BUSY`（設計最重要リスク R-1）。
  - 完了条件: 稼働構成で cron を回すプロセスが厳密に 1 つであることを確認。

- [x] **P0-3 nginx strangler のポート整合とルーティング明示** — 済（`deploy/nginx/yuuka.conf`：現行 Rust 直起動では本ファイル不使用＝Tunnel→:7854 直達である旨を冒頭バナーで明記。経路 A 用としては catch-all と `/ws/chat` を `yuuka_node` に戻し「既定 Node・移行済みだけ Rust」の fail-safe に整合。Rust 並走時は `PORT=7900` 起動が前提と明示）
  - 内容: `deploy/nginx/yuuka.conf` の `yuuka_rust` upstream は `:7900` を指すが、コンテナは `:7854` を listen。catch-all と `/ws/chat` は Rust を向くのにコメントは「fail-safe で Node」と矛盾。ポートを合わせ、各 location の向き先を実態に一致させる（または Cloudflare Tunnel 直叩きでこのファイルを使わない旨を明記）。
  - なぜ: 現状のまま適用すると catch-all が存在しないポートを叩き **502**。
  - 完了条件: 実際に流すトラフィックの経路が設定と一致し、意図通り Node/Rust に分岐する。

- [x] **P0-4 起動前に DB を事前シードする運用を明記** — 済（`deploy/README.md` 「DB は起動前に必ず存在させる」。Rust は `SQLITE_OPEN_CREATE` を付けず無ければ起動失敗＝旧 Node 作成の `data/yuuka.db` を再利用する手順を記載）
  - 内容: Rust は存在しない DB を作らない（`open_conn` がエラー）。fresh インスタンスは Node が作成した `data/yuuka.db` を再利用する。
  - 完了条件: 初回起動手順書に「DB 事前作成」を記載。

---

## P1 — 致命ブロッカー（単独ほぼ本番に不可欠）

- [~] **P1-1 認証の発行経路** — **資格情報ログインは実装済み・OAuth は残（P2-A へ）**
  - 済（2026-07-09d）: `/api/login` `/api/logout` `/api/register` `/api/register/verify` `/api/setup` `/api/setup/status` `/api/users`（Node `authRoutes.ts` パリティ）を `crates/yuuka-auth/`（`routes.rs`・`AuthRuntime`）に実装。`SessionStore` に**発行**（`create`/`destroy` + in-memory フォールバック）を追加し、`CompositeAuth`（検証）と**同一ストアを共有**（`main.rs` で `sessions.clone()`）。bcrypt cost 12（`$2b$`・Node bcryptjs 相互運用・`spawn_blocking`）、`verifyPasswordConstantTime`（不在ユーザーもダミー比較でタイミングオラクル対策）、パスワードポリシー（8 文字/2 種/denylist fail-open・UTF-16 長）、招待コード（`is_valid`/atomic consume/起動時 seed）、監査ログ、DM チャレンジ登録（`PendingStore` + `RegistrationDm` ポート・`NullRegistrationDm` 縮退）、レート制限（login lockout・register-send window）、`ConnectInfo`+XFF クライアント IP、Cookie 発行（`setSessionCookie` パリティ）。config に `INVITE_CODES`/`ADMIN_DISCORD_IDS` 追加。統合テストで **login/setup → セッション発行 → /api/me 200** を確認。
  - 残: **Google/Discord OAuth フロー**（`settingsRoutes.ts` の url + callback・`GOOGLE_CLIENT_ID`/`GOOGLE_CLIENT_SECRET`）は未移植 → **P2-A（設定系ルート）へ移す**。
  - 残（P1-3 と一体）: `register` の確認コード DM は `NullRegistrationDm` のため現状 502。Discord live 化で `impl RegistrationDm for DiscordMessenger`（既存 `send_registration_code_dm` へ委譲・notify_bridge と同型）を足し `Arc<DiscordMessenger>` を注入すれば届く。
  - 残（経路 A のライブ確認）: 共有 Redis 稼働下で Node が発行した Cookie を Rust が検証、Rust が発行した Cookie を Node が検証、の相互運用を実 Redis で確認（キー書式・sha256hex・camelCase JSON は一致済み）。

- [~] **P1-2 会話経路** — **オーケストレーション中核（実 TurnProcessor）は実装済み・transport 配線が残**
  - 済（2026-07-09d・新 `crates/yuuka-orchestrator`）: **チャットオーケストレーション層 + 実 `TurnProcessor`** を実装。`ChatEngine::secretary_turn`＝Node `processMessage`（秘書経路）パリティ: リッチ返信フラグ → ユーザー発言永続化（`describeIncomingMessage`）→ 直近 15 件を古い順ロード → `contents` 組立（連続同一 role を `\n` 結合・添付 inline data）→ `buildSystemInstruction`（DEFAULT_PERSONA/ペルソナ + 情報保存/承認/リッチ返信/音声/ファクトチェック/機能一覧/システムルール[現在日時・**未実行の完了報告禁止**]を verbatim 移植）→ ユーザーの Gemini キー復号（`SystemCrypto`・秘書経路は `users` の鍵）→ **FC ループ**（既存 `run_function_calling_loop`）→ アシスタント応答を必ず永続化 → `TurnReply`。`message_log`/`user`/`persona` repo 新設（`message_logs` の add/recent_context[floor=`system_settings` `context_floor:`]/clear_context）。`impl TurnProcessor for ChatEngine`（`process_secretary`/`parse_receipt` 実装）。`GeminiFactory` トレイトで fake backend 注入 → 統合テストで user 発言→履歴→鍵復号→FC ループ→assistant 保存→reply を通し検証。
  - 済 **(1) `/ws/chat` WebSocket transport**（2026-07-09d・`crates/yuuka-supervisor/src/ws.rs`）: axum WS upgrade + **Bearer（desktop token）認証**（`AuthenticatedUser` extractor）+ `?botId=` 束縛/`has_bot_access` 検証（未指定 system_default）+ WS-native ping/pong 30s。フレームは `clients/desktop/src/model.rs` 契約と一致: 受信 msg/reset/ping/interaction、送信 **ready/status/done/error**。`ready` は `listBotsForUser`（bots + bot_shares active）を BotInfo 化・束縛 Bot は合成フォールバック（`toBotInfo` パリティ）。msg → Gemini キー事前チェック（`no_gemini_key`）→ `ChatEngine::secretary_turn` → status（thinking/writing）→ done。reset → `clear_context`。送信は split+mpsc の writer タスクへ集約。`build_app` に `ws_routes` 追加・main.rs で `ChatEngine`（tool registry + crypto + db）構築 + 配線。**live 統合テスト**（実 WS クライアント → 実サーバ・fake Gemini）で ready→msg→done を通し検証。
  - 残 **(2) 縮退シームの本体化（後続・任意）**: ターンプランナー・シナプス想起（Phase H daemon）・**非同期配信**（interim/push・deferred）・**interaction 配信**（コンポーネント/update）・**能力ゲート**（現状全ツール露出＝P2-B）・返信チェーン・上限は raw バイト長判定（現状フレーム長概算）。
  - 残 **(3) Discord 経路**: `ChatEngine`（`Arc<dyn TurnProcessor>`）を P1-3 の `DiscordManager` へ注入。汎用モード（guild/owner DM・Bot 専用キー）の `process_guild`/`process_bot_dm` は現状未実装（P1-3 で実装）。
  - 完了条件: **デスクトップから 1 往復の会話が成立しツール呼び出しが実行される**（`/ws/chat` で成立・Web ダッシュボードは WS チャット未使用のため対象外）。残は Discord 経路（P1-3）。

- [x] **P1-3 Discord を実起動し、Supervisor 配下へ配線する** — **着地（2026-07-09e）+ parity レビュー済み（2026-07-10）**
  - 済: DB ポート実装（`DbBotDirectory`/`DbMembership`/`InMemoryRateLimiter`）+ 汎用モード（`process_guild`/`process_bot_dm`）+ `main.rs` 配線（`DiscordManager.prepare` → `DiscordTenantService` を Supervisor 監督下・`YUUKA_RUST_DISCORD` env ゲート既定 off）。parity レビュー確定 6 件を修正（本ファイル冒頭「2026-07-10 の成果」参照）。
  - 済（2026-07-10b・縮退シーム 3 件）:
    - [x] **利用申請/決定の Discord DM**（owner へ申請通知・applicant へ承認/却下通知）。interaction ハンドラが outcome 露出の id で既存ヘルパを呼ぶ（`MemberDmSender` 注入・fire-and-forget）。
    - [x] **汎用モード LLM エラーの文言分類**（rate-limit/server-error 別の ⚠️・秘書/汎用で別文言）。`classify_gemini_error` で `Ok(TurnReply::text)` を返す（履歴非保存）。
    - [x] **能力ゲート適用**（秘書 × 汎用モードの経路 × 能力集合）。残: ユーザー別 `enabledModules`（P2-A・下記 P2-B 参照）。
  - 残（実機）: 実トークンでのメンション/DM 応答確認・Node bot 停止のカットオーバー（二重処理回避）。

- [x] **P1-4 通知配信の橋渡し（Messenger → Notifier）を実装する** — **配線済み（P1-3 と一体・2026-07-09e）+ DM フォールバック修正済み（2026-07-10）**
  - 済: `crates/yuuka-discord/src/notify_bridge.rs`＝`impl yuuka_services::Notifier for DiscordMessenger`（`NotifyTarget`↔`DeliverTarget` 変換＝Default→DM・Channel 透過、空本文 false、`TurnReply::text` 化して `send_to_user` へ委譲）。孤児規則により discord 側に実装（services→discord 逆依存なし＝非循環）。target 写像を単体テストで凍結。`main.rs` の `NullNotifier`→`Arc<DiscordMessenger>` 差し替え済み。
  - 済（2026-07-10 parity 修正）: `send_to_user` の Channel 解決失敗時に DM フォールバック（Node `sendToUser`）。これが無いとチャンネルを閲覧不可のリマインダーが送信されず reminder が永久リトライ状態になっていた。
  - 完了条件: 期限到来リマインドが実 Discord チャンネルへ届く（実機確認は P1-3 のカットオーバー時）。

- [x] **P1-5 秘密情報の暗号層（Argon2id + AES-256-GCM）を実装する** — 済（新 `crates/yuuka-crypto`）。scrypt システム鍵 + Argon2id ユーザー鍵 + AES-256-GCM を Node `src/utils/crypto.ts` と **バイト単位パリティ**で実装（Node 実出力のゴールデンベクタ `golden_parity_with_node` で凍結）。`config` が `YUUKA_ENCRYPTION_SECRET`/`_NEW` を `SecretString` で読込。鍵ローテ（`rotate_secret_key`＝Node `ENCRYPTED_COLUMNS` パリティ）を supervisor 起動時に **writer actor 上で 1 回**実行（R-2 遵守）。**残（消費側の配線は P2）**: credential register/decrypt ルート・Discord/Gemini トークン復号は `SystemCrypto`/`decrypt_text` を呼ぶだけ（本層で提供済み）。
  - 内容: **Rust 全クレートが読む env は `YUUKA_RUST_CRON` ただ 1 つ**。`YUUKA_ENCRYPTION_SECRET` / `YUUKA_ENCRYPTION_SECRET_NEW`（鍵ローテ）/ `GOOGLE_CLIENT_SECRET` は未読。復号層が「本クレート外」のまま存在しない（`yuuka-credential` は register/decrypt を deferred）。
  - なぜ: 保存済み資格情報・Discord トークン・Gemini キー・Google リフレッシュトークンの**復号が全滅**。at-rest 秘密に依存する機能が全て非機能。
  - 対象: 新規 crypto crate（Argon2id 鍵導出 + AES-256-GCM）、`crates/yuuka-core/src/secrets.rs`（現状 in-memory `SecretString` ラッパのみ）、`crates/yuuka-credential/`、config で `YUUKA_ENCRYPTION_SECRET` 必須化（Node は未設定なら起動しない）。
  - 完了条件: DB の `encrypted_password/iv/auth_tag` を復号でき、鍵ローテ（`_NEW`）も Node パリティ。

- [x] **P1-6 未コミットの Batch 4/5/6 修正を確定コミットする** — 済（`61934e8`・P0-1 と同一コミット）
  - 内容: working tree の M-1（認証縮退 Bearer 継続 + `tracing::warn!`）/ M-2（413 区別・空ボディ `{}` 化）/ M-4（Auth::Backend のみ Transient）/ M-5（writer `catch_unwind` panic 隔離）/ M-6（連鎖削除 parity）を含む 12 ファイル差分。レビュー承認済み（[review-2026-07-09-batch4-6.md](review-2026-07-09-batch4-6.md)）。
  - 完了条件: コミット済み。P0-1 と同一コミットに含めてよい。

- [x] **P1-7 `cargo deny` を緑に戻す** — 済（`19443ae`・選択肢 (i) 採用: crawler/synapse を members→exclude へ。`cargo deny check` exit 0）
  - 内容: 補助クレート由来の 3 系統を解消 —（a）`fxhash` unmaintained RUSTSEC-2025-0057（scraper←`yuuka-crawler`）、（b）MPL-2.0 未許可 ×4、（c）`yuuka-crawler`/`yuuka-synapse` の license 欄欠落で unlicensed。
  - なぜ: 計画（`Cargo.toml` 冒頭）は「最終 Phase H まで crawler/synapse を members に入れない」としていたが取り込まれており、workspace ゲートが赤。
  - 選択肢: (i) 両クレートを members から exclude に戻す（計画準拠・最速）、(ii) license 欄追加 + `deny.toml` に MPL-2.0 許可追加 + fxhash を `rustc-hash` へ差し替え or advisory 個別許可。
  - 完了条件: `cargo deny check` exit 0。

---

## P2 — 機能パリティ（Node にあって Rust に無い）

### P2-A Web ルート（57/152 → 埋める）

> 2026-07-13（`b77506d`）で 9 ドメイン CRUD のうち todo/finance/timeline/personal/playbook を拡張済み（下記ドメイン別を参照）。2026-07-14 に管理系（admin）を実装済み。以下の設定・Bot・MCP 系は未着手。

- [x] 管理系 `/api/admin/*`（default-bot/token・stats・system-settings・users/role/delete・audit-logs・bots/suspend/unsuspend・invite-codes CRUD 計 15）— Node `adminRoutes.ts` パリティ（新規 `yuuka-admin` クレート・2026-07-14）。**残**: Discord runtime 効果（Bot 再起動/停止/`isRunning`）は `BotRuntime` シーム＝gateway 配線時に実装注入
- [~] 設定系 `/api/settings/*`（`settingsRoutes.ts`）: 済（2026-07-14・`yuuka-settings`）＝profile/password/delete-account/user/gemini/backup の 6。**残（Google Drive/Calendar HTTP 依存・別増分）**: `/api/status`（多ドメイン集計 + カレンダー）・discord GET/POST（Bot トークン + 再起動）・google OAuth url/callback・calendars・backup/trigger
- [ ] Bot 管理 `/api/bots` `/profile` `/shares*` `/sync-discord` — `botRoutes.ts`
- [ ] Bot 属性 `/api/bots/attributes` `/modules` `/presets` `/assistant/*`（~13）— `botAttributeRoutes.ts`
- [ ] MCP `/api/mcp-servers*` `/proxy/mcp/:id/mcp`（~8）— `mcpRoutes.ts`
- [ ] Webhooks `/api/webhooks*` `/hook/:token`（~6）— `webhookRoutes.ts`
- [ ] Integrated `/api/integrated/*`（google accounts/calendars/grants・bot start/stop/restart 等 ~12）— `integratedRoutes.ts`
- [x] Device/Desktop `/api/auth/device/*`（2026-07-15c・`50992fa`・RFC8628 フロー）`/api/devices*`（2026-07-14g・deviceMgmt）`/api/desktop/*`（2026-07-14i・配布）— `deviceAuthRoutes.ts` 他すべて移植済
- [ ] Delivery `/api/briefing-config` `/briefing/test` `/report-configs*` — `deliveryRoutes.ts`
- [ ] Member requests `/api/bots/member-requests*` — `memberRequestRoutes.ts`
- [ ] Persona marketplace `/api/personas/marketplace*` `/activate` `/import` `/publish` `/admin/personas/*`（現状 save/delete/list のみ）
- [ ] `/api/status`（ヘルスチェック）・`/api/setup/status`（コメントで「後続」と言及したまま未配線）
- [~] ドメイン別の残ルート（2026-07-13 `b77506d` で todo/finance/timeline/personal/playbook を拡張）:
  - [x] todo（9/9）: detail / gantt / progress / someday / update を追加（子孫は再帰 CTE でスコープ収集・`PriorityUpdate` 3 値 parity）
  - [~] finance（8/9）: budget-limits / plans/* / **月次集計（total・incomeTotal・breakdown・trend）**（2026-07-14）を追加。**残**: upload-receipt（receipt OCR = Gemini vision + supervisor 層配線が必要・deferred）
  - [x] timeline（8/8）: plan/*・**media（`/api/timeline/media*` base64 アップロード + 認証付き配信）**・**cross-domain 副作用（`type=expense`→expenses 二重登録・`type=task_done`→todos 完了）**（2026-07-14）を追加
  - [x] personal（6/6）: clipboard（GET/delete）/ context-note に加え **`addClipboardEntry` ツール**（2026-07-14）。TTL 一括削除 cron（`deleteExpired`）は `ClipboardCleanupService` で稼働済み
  - [x] credential（3/3）: register 実装済（2026-07-15a・crypto 注入 `routes_with`・owner-Bot 一括付与・GET 許可フィルタ・delete 掃除）
  - [x] playbook（8/8）: runs / schedules/* に加え **cron 実行エンジン（`PlaybookScheduleService`・EveryMinute tick・due 判定 + run 記録 + 通知）**（2026-07-14）。**マクロ定期実行が実際に走る**（`YUUKA_RUST_CRON=1` 時・P2-C 参照）。route 層の cron 式妥当性検証のみ deferred
  - （schedule 3/3・reminder は完了）

### P2-B Gemini ツール（27/87 → 埋める）

> **注意**: 2026-07-13（`b77506d`）は **Web ルート（P2-A）のみ**を拡張し、多くの Gemini ツール（LLM が function-calling で呼ぶ側）は未追加のまま。よって finance budget-limits/todo detail 等は **ダッシュボードからは操作できるが LLM からはまだ呼べない**。2026-07-14 に clipboard 3 本を追加し、timeline は expense/task_done を tool から実行可能にした（27/87）。

- [x] todo: 13/13 完成（2026-07-14＝subtask/update/progress/detail/tags 2/guide + **editTodoTags/stopTodoRoutine**〔新 repo update_tags/stop_routine〕）。organizeTaskPriorities/applyTaskPriorities/getRecentActionHistory は Node 側で LLM/別基盤依存のため範囲外
- [x] finance: 14/14 完成（2026-07-14＝月次集計2/予算3/支払い予定 list-add-settle-cancel4/findSettlementCandidates/**linkPlannedPaymentTodo/linkPlannedPaymentReminder**〔cross-domain＝同一DBへ直接SQL・link_todo/link_reminder repo〕）
- [~] timeline: addTimelineRecord は expense/task_done の cross-domain 副作用に対応済み（2026-07-14）。**残**: createDayPlanBlock/listDayPlan/deleteDayPlanBlock（day_plan_blocks 操作）・tool 経由 media の Discord 添付 URL 取得（reqwest 依存）
- [x] personal: clipboard(3) + **context-note 3（getContextNote/setContextNote/appendContextNote・2026-07-14＝既存 ContextNoteRepo へ配線・上限 10,000 文字検証・append は改行連結）** + searchContacts（`SearchContactsTool`・name/relationship/notes/tags の LIKE 部分一致・実装済。※ `tools.rs` 冒頭コメントの「deferred」は stale）
- [~] credential: **add/update/list/delete 済 + 許可配線済**（2026-07-15a＝`SystemCrypto::encrypt_for_user`・salt=users.salt・`build_tool_registry(db,crypto)` 注入・長さ検証/部分更新。list は `bot_credential_access` 許可フィルタ・add は応対 Bot へ grant・delete は grant 掃除。**パスワードは trim せず保存**〔`arg_password`〕・`url=""` は URL 削除〔`arg_url_update`〕）。**残**: browserFillCredential（平文復号 + browser 依存）
- [ ] browser 一式（searchWeb/fetchDynamicPage/takePageScreenshot/browserInteractive ~9）— 対応クレート無し
- [ ] chart（sendChart）
- [~] briefing（2026-07-14＝**configureReport/getBriefingConfig/configureBriefing**〔新 yuuka-briefing クレート・report/briefing 部分更新 upsert + 設定読取。configureBriefing は cron 検証 + add_news_feed の SSRF ガード〔内部/ループバック/リンクローカル/メタデータ拒否〕+ フィード add〔重複無視〕/remove〔部分一致〕+ weather_lat/lng/地名/キーワード部分更新〕）。**残**: runBriefingNow（サービス本体＝天気/RSS 取得の配信実行）
- [x] richContent（showRichContent・常時 on のコア）— 2026-07-14＝新クレート `yuuka-richcontent`・core（capability=None・両経路露出）・Embed 配管新設（core `ResponsePart::Embed`→engine `TurnReply.embeds`→Discord twilight / desktop APIEmbed JSON）。詳細は冒頭「2026-07-14v」
- [x] botAssistant（2026-07-14＝**個人/共有ノート 6 + メンバー管理 3**〔addBotMember/listBotMembers/removeBotMember・extract_user_id メンション解析・remove は自己/owner のみ・yuuka-botassistant クレート〕）。
- [x] conversation: **summarizeConversationTopic**（2026-07-14＝新クレート `yuuka-conversation`・cap=memory・秘書経路のみ。詳細は冒頭「2026-07-14u」）
- [ ] **MCP 動的ツール**（`McpProvider` は未実装。`yuuka-tools/src/lib.rs` で deferred）
- [~] **capability ゲート適用**: 済（2026-07-10b）＝経路（秘書/汎用モード）× 能力集合で `NativeProvider.list()` を絞り込み（`ToolExposure`/`ctx.mode`+`ctx.capabilities`・Node `parseCapabilities`+`getFunctionModulesForCapabilities`/`getGuildAssistantFunctionModules` パリティ）。**残**: ユーザー別 `enabledModules`（`resolveEnabledModulesForUser`＝`bot_user_modules`/`bots.enabled_modules` の selectable モジュール絞り込み・Node の第 2 次元）が未移植＝module 選択 UI 設定に連動（本項の完了はこの実装で）。

### P2-C 常駐サービス（7 実装 + 3 予約シーム + 1 欠落）

- [ ] report（日報/週報）— 予約シーム no-op（Gemini aux-gen + charts 依存）
- [ ] briefing（朝報/天気/RSS）— 予約シーム no-op（weather/RSS HTTP + SSRF ガード依存）
- [ ] backup（Google Drive）— 予約シーム no-op。**自動バックアップが走らない**（per-user Drive OAuth 依存）。実データを扱うなら要注意。
- [x] playbook-schedule（マクロ自動実行）— **実装完了（2026-07-14）**。`PlaybookRunner` ポート + `PlaybookScheduleService`（EveryMinute tick・cron due 判定・run 記録・通知）+ cross-user scan（yuuka-playbook `cron.rs`）+ `PlaybookRunnerAdapter`（main.rs で `ChatEngine` へ橋渡し・循環回避）。`YUUKA_RUST_CRON=1` でマクロ定期実行が実際に走る。tick モデルは復帰直後に取りこぼしを最大 1 回 catch-up（Node は catch-up 無し・意図的差分・有界）。
- [ ] synapse engine（認知想起）— **予約シームですらなく完全欠落**（Node は外部 Rust synapse daemon を spawn）。Phase H の daemon 吸収で対応。

### P2-D 設定キーの取り込み

- [ ] `GOOGLE_CLIENT_ID` / `GOOGLE_CLIENT_SECRET`（OAuth）
- [ ] `INVITE_CODES`（起動時シード）
- [ ] `ADMIN_DISCORD_IDS`（初期 admin bootstrap）
- [ ] `REMINDER_CRON` 等の cron スケジュール上書きキー（現状 Rust は自前スケジュール固定）

---

## P3 — 品質 / 運用衛生

- [x] **P3-1 CI ゲートの新設** — 済（2026-07-14・`.github/workflows/rust-ci.yml`＝fmt --check + clippy -D + build --release + test + cargo-deny。push[develop/main/feature/rust-rewrite] + PR で起動）。**残**: `gen-types --check` drift（xtask に cargo alias 未整備・別途）
- [x] **P3-2 `cargo fmt` 差分の解消** — 済（2026-07-14・`cargo fmt --all` で 103 ファイル一括正規化・P3-1 CI と同一コミット。以後 CI の fmt --check で常時緑を強制）
- [ ] **P3-3 残レビュー指摘 M-7〜M-11（fail-closed）**:
  - [ ] M-7 priority 正規化 + float `2.0` 受理幅
  - [ ] M-8 finance amount 検証
  - [x] M-9 reminder `trigger_at` 正規化 — 済（2026-07-12・B4）。`datetime::to_db_datetime` を repo 境界 + tool/web route で適用。**残**: 過去日時の拒否・繰り返しの次回自動前進は未移植（低・own-user・cron next は上位 crate 依存）
  - [x] M-10 credential 許可フィルタ（bot_credential_access）— **配線完了**（2026-07-15a・`d2a7c5b`/`767e265`）。repo 6 メソッド（`92be802`）に加え register の owner-Bot 一括付与 + `grant_to_owner_bots` / GET 許可フィルタ / delete 掃除 / addCredential 応対 Bot 付与 / deleteCredential 掃除 / crypto 注入を配線。敵対的レビューで確定 5 件是正（パスワード非 trim 等）
  - [ ] M-11 persona 適用中の delete 拒否
- [x] **P3-4 README 冒頭の古い記述を修正** — 済（[README.md](README.md) の「実装はまだ開始していない」を Phase 0〜5 着地の現況＋remaining-work.md 参照に更新）。
- [x] **P3-5 `/api/me` の DB 再取得 + 404 分岐** — 済（`yuuka-web/src/routes.rs`：セッション解決後に `SELECT username, role FROM users WHERE discord_id` を read pool で再取得し、消失時 404 `{success:false,message:"ユーザーが見つかりません。"}`＝Node parity。role は DB 権威。テスト `me_returns_404_when_user_deleted_from_db` 追加・既存 200 テストは users 行を seed）。
- [ ] **P3-6 index.html への google-site-verification meta 注入**（deferred・`static_files.rs`）。
- [x] **P3-7 Docker イメージのスリム化** — 済（2026-07-12）。`[profile.release]` に strip+thin-LTO（バイナリ 311MB→**24MB**）。`Dockerfile` を全面刷新し runtime=debian-slim に **yuuka バイナリ + `dist/public` のみ**同梱（Node/node_modules/chromium/フォント/dist/index.js/crawler/synapse/desktop.exe/docs 非同梱）。ビルド段は rust(yuuka のみ)+frontend(vite のみ)。**イメージ 59MB**・docker build/run で疎通確認済み。frontend ビルドのみ Node 段（vite）が必要なのは不変。
- [ ] **P3-8 LOW 群**（`reminders/delete` 撤去の是非・float priority 受理幅ドキュメント化・repo docstring stale 等）。

---

## 4. 参考：フェーズ対応（[PLAN.md](PLAN.md) §11）

| フェーズ | 内容 | 現在地 |
|---|---|---|
| Phase 0 | 契約凍結（core/db/types） | ✅ 完了 |
| Phase 1 | Web/認証/静的/ドメイン CRUD | 🔶 ほぼ完了（P3 の残指摘・P2-A の深掘り残） |
| Phase 2 | Gemini + tools + 全ドメインツール登録 | 🔶 FC ループ + 24 ツール（上位層・残ツール未） |
| Phase 3 | Discord（twilight） | 🔶 転送層のみ（live 未起動 = P1-3） |
| Phase 4 | 常駐サービス | 🔶 6 実装 / 4 予約シーム（配信橋渡し未 = P1-4） |
| Phase 5 | Dockerfile/nginx カットオーバー | 🔶 配管切替済（整合は P0-3） |
| Phase D/E/G | gemini 上位層 / discord live / services 本体 | ⬜ 主に P1〜P2 |
| Phase H | synapse/crawler の daemon 吸収 + JoinSet 全体監督 | ⬜ 未 |

---

## 5. 完了チェックリスト（経路別）

### 経路 A — strangler 並走カナリア（最短で限定検証）
- [x] P0-1 V17 コミット + 凍結（`61934e8`）
- [x] P0-2 cron 片側専任（`deploy/README.md`）
- [x] P0-3 nginx ポート整合（`deploy/nginx/yuuka.conf`）
- [x] P0-4 DB 事前シード（`deploy/README.md`）
- [ ] 共有 Redis セッション鍵/キー書式が Node と一致することを実 Redis で確認（**未検証・要確認**）
- [ ] 移行済み CRUD サブセットのみを Rust へ向け、それ以外は Node（fail-safe）
- → これで「非暗号フィールドの CRUD を Rust が捌く」限定 near-prod 検証が可能。

### 経路 B — 単独ほぼ本番（Node 撤去）
- [ ] P1-1〜P1-7 を全て解消
- [ ] P2-A/B/C/D を実用十分な水準まで
- [ ] P3-1 CI ゲート常時緑
- [ ] 実 Redis 稼働下の Cookie 検証ライブ確認（環境に Redis 必要）
- [ ] push / prod deploy / `YUUKA_RUST_CRON` 本番 ON（= Node cron 停止カットオーバー）は**ユーザー最終判断**

---

## 付録：主要ファイル早見

| 関心事 | ファイル |
|---|---|
| 管理系 API（`/api/admin/*`・BotRuntime シーム） | `crates/yuuka-admin/src/{lib,routes,repo,dto}.rs`, main.rs `AdminRuntime`/`NullBotRuntime` |
| 設定系 API（`/api/settings/*`・セッション再発行） | `crates/yuuka-settings/src/{lib,routes,repo,dto}.rs`, main.rs `SettingsRuntime` |
| セッション一括失効 | `crates/yuuka-auth/src/session.rs`（`destroy_all_for_user`） |
| 設定系が再利用する auth 公開 API | `crates/yuuka-auth/src/{users.rs(hash_password),routes.rs(build_session_cookie/SessionCookieToken),desktop.rs(revoke_all_for_user)}` |
| 起動配線（web のみ監督・discord/cron 未配線） | `crates/yuuka-supervisor/src/main.rs` |
| 予約シーム 3 no-op（report/briefing/backup） | `crates/yuuka-services/src/deferred.rs`, `.../lib.rs` |
| playbook 実行エンジン（実装済み） | `crates/yuuka-services/src/{playbook_schedule,turn}.rs`, `crates/yuuka-playbook/src/cron.rs`, main.rs `PlaybookRunnerAdapter` |
| timeline media | `crates/yuuka-timeline/src/media.rs`, `.../routes.rs`, `crates/yuuka-web/src/config.rs`（`media_dir`） |
| NullNotifier | `crates/yuuka-services/src/notifier.rs` |
| Discord 転送層（不活性） | `crates/yuuka-discord/src/{manager,message_flow,ports}.rs` |
| 未使用アダプタ | `crates/yuuka-supervisor/src/discord.rs` |
| config ローダ（暗号 env 未読） | `crates/yuuka-core/src/config.rs`, `.../secrets.rs` |
| 暗号 deferred | `crates/yuuka-credential/src/{lib,routes,tools}.rs` |
| V17 baseline（未コミット修正） | `crates/yuuka-db/migrations/V17__baseline.sql`, `.../src/schema.rs` |
| nginx strangler | `deploy/nginx/yuuka.conf` |
| Dockerfile（`YUUKA_RUST_CRON=1` 焼込・Rust CMD） | `Dockerfile` |
| Node パリティ基準 | `src/index.ts`（起動）, `src/server/`（ルート）, `src/functions/`（ツール）, `src/services/`（cron）, `src/bot.ts`（Discord）, `src/gemini.ts`（FC + 上位層）, `src/server/chatWebSocket.ts`（WS） |
