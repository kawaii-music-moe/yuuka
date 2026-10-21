# Node → Rust 差分の対応状況（2026-10-22）

## 基準と読み方

[#97](https://github.com/kawaii-music-moe/yuuka/issues/97) の対応表。`develop` の `97566f0` を基準に、マージ履歴と実装の接続先を照合した。
[2026-10-08の棚卸し](2026-10-08-feature-parity.md)にある「現在」「未コミット」「未接続」は当時の記録を含み、本表を現在の対応状況として参照する。
HTTP 180組・会話ツール85件のTSVも元の監査時点の入口対応表として保持する。入口の存在は挙動の同等性の保証ではない。

R01〜R27の移行差分にはすべて修正がマージ済み。D01〜D08も整理方針を実装済み。R16の失敗処理（#182）は[#200](https://github.com/kawaii-music-moe/yuuka/pull/200)、D06の登録時検査（#184）は[#202](https://github.com/kawaii-music-moe/yuuka/pull/202)、レート制限判定の集約（#192）は[#203](https://github.com/kawaii-music-moe/yuuka/pull/203)で追加修正もマージ済み。
他の作業ブランチの変更を完了扱いに含めない。IssueのOPEN/CLOSEDとdevelopへの実装のマージ状況は区別する。

## R01〜R27

「修正済み」は対応PRがdevelopにマージされたことを表す。外部サービスとの実通信や本番検証の完了を意味しない。各PRと個別監査に検証範囲を記載している。

| ID | 状態 | 機能 | 実装PR | 根拠・適用範囲 |
|---|---|---|---|---|
| R01 | 修正済み | Webhook受信 | [#157](https://github.com/kawaii-music-moe/yuuka/pull/157) | [yuuka-webhook/src/processor.rs](../../crates/yuuka-webhook/src/processor.rs)：署名・リプレイ検査、通知、ToDo/リマインド化、受信履歴を実装。supervisorでRealWebhookProcessorを注入。 |
| R02 | 修正済み | テスト配信 | [#165](https://github.com/kawaii-music-moe/yuuka/pull/165) | [yuuka-services/src/delivery.rs](../../crates/yuuka-services/src/delivery.rs)：NotifierDeliveryRunnerで定時配信と同じ生成・配信経路を使用。 |
| R03 | 修正済み | 共有招待DM | [#168](https://github.com/kawaii-music-moe/yuuka/pull/168) | [yuuka-discord/src/manager.rs](../../crates/yuuka-discord/src/manager.rs)：WebもDiscordMessengerのボタン付きDM送信を使用。 |
| R04 | 修正済み | 利用申請DM | [#168](https://github.com/kawaii-music-moe/yuuka/pull/168) | [yuuka-discord/src/manager.rs](../../crates/yuuka-discord/src/manager.rs)：Webの申請・承認結果もDiscordと同じ送信ポートを使用。 |
| R05 | 修正済み | ToDo自動タグ | [#190](https://github.com/kawaii-music-moe/yuuka/pull/190) | [yuuka-todo/src/auto_tags.rs](../../crates/yuuka-todo/src/auto_tags.rs)：DBトリガーで全作成経路を予約し、本人の補助生成を利用。明示タグと世代競合を保護。 |
| R06 | 修正済み | ターン計画 | [#196](https://github.com/kawaii-music-moe/yuuka/pull/196) | [yuuka-orchestrator/src/turn_plan.rs](../../crates/yuuka-orchestrator/src/turn_plan.rs)：能力で絞ったカタログから計画し、実行ツールの宣言は全体を維持。 |
| R07 | 修正済み | 途中応答・後続配信 | [#196](https://github.com/kawaii-music-moe/yuuka/pull/196) | [yuuka-orchestrator/src/engine.rs](../../crates/yuuka-orchestrator/src/engine.rs)：重い計画または12秒経過で途中応答し、本体を続行して最終結果を1回配信。 |
| R08 | 修正済み | 返信チェーン | [#196](https://github.com/kawaii-music-moe/yuuka/pull/196) | [yuuka-orchestrator/src/message_log.rs](../../crates/yuuka-orchestrator/src/message_log.rs)：Discordと会話ログの取得結果をIDで統合し、プロンプトと想起へ渡す。 |
| R09 | 修正済み | WS操作 | [#195](https://github.com/kawaii-music-moe/yuuka/pull/195) | [yuuka-supervisor/src/ws.rs](../../crates/yuuka-supervisor/src/ws.rs)：共通のボタン判定を使い、配信済みボタンだけ処理してupdate/pushを送る。 |
| R10 | 修正済み | 朝報ニュース要約 | [#186](https://github.com/kawaii-music-moe/yuuka/pull/186) | [yuuka-briefing/src/service.rs](../../crates/yuuka-briefing/src/service.rs)：本人の補助生成で要約し、失敗時は見出し列挙へ縮退。 |
| R11 | 修正済み | 日報・週報要約 | [#187](https://github.com/kawaii-music-moe/yuuka/pull/187) | [yuuka-services/src/report.rs](../../crates/yuuka-services/src/report.rs)：本人・同じBotの活動と会話を補助生成へ渡し、失敗時は生データへ縮退。 |
| R12 | 修正済み | 検索資料注入 | [#174](https://github.com/kawaii-music-moe/yuuka/pull/174) | [yuuka-orchestrator/src/system_prompt.rs](../../crates/yuuka-orchestrator/src/system_prompt.rs)：検索資料をビルド時に埋め込み、システムプロンプトへ注入。 |
| R13 | 修正済み | 会話添付の保存 | [#191](https://github.com/kawaii-music-moe/yuuka/pull/191) | [yuuka-timeline/src/media.rs](../../crates/yuuka-timeline/src/media.rs)：本人の添付を順序・冪等性付きで保存。配信認可と実体バックアップを接続。 |
| R14 | 修正済み | 既存支出による消込 | [#152](https://github.com/kawaii-music-moe/yuuka/pull/152) | [yuuka-finance/src/repo.rs](../../crates/yuuka-finance/src/repo.rs)：同じ利用者/Botの既存支出へexpense_idで紐付け、再記帳せずToDoを完了。 |
| R15 | 修正済み | 消込・取消の通知停止 | [#154](https://github.com/kawaii-music-moe/yuuka/pull/154) | [yuuka-finance/src/repo.rs](../../crates/yuuka-finance/src/repo.rs)：予定更新と同じトランザクションで関連pendingリマインドを取消。 |
| R16 | 修正済み | 記帳後の予算・候補 | [#171](https://github.com/kawaii-music-moe/yuuka/pull/171)、[#200](https://github.com/kawaii-music-moe/yuuka/pull/200) | [yuuka-finance/src/tools.rs](../../crates/yuuka-finance/src/tools.rs)：budget_usageとsettlement_candidatesを復旧。補足取得失敗時も記帳成功と支出IDを返し、unavailable_supplementsと再記帳不要の案内を添える。消込候補の未取得はnullで候補なしと区別する。 |
| R17 | 修正済み | 既定の通知先 | [#158](https://github.com/kawaii-music-moe/yuuka/pull/158) | [yuuka-services/src/notifier.rs](../../crates/yuuka-services/src/notifier.rs)：Defaultを本人の設定で解決し、addReminderにも既定の宛先を保存。 |
| R18 | 修正済み | 過去リマインド保護 | [#159](https://github.com/kawaii-music-moe/yuuka/pull/159) | [yuuka-reminder/src/tools.rs](../../crates/yuuka-reminder/src/tools.rs)：会話ツールの過去の単発は拒否、繰り返しは次回へ補正。管理HTTPは旧Nodeと同じく対象外。 |
| R19 | 修正済み | cron検証 | [#160](https://github.com/kawaii-music-moe/yuuka/pull/160) | [yuuka-core/src/cron_util.rs](../../crates/yuuka-core/src/cron_util.rs)：登録と実行に同じcronerパーサを使い、発火日がない式も拒否。 |
| R20 | 修正済み | Botプロフィール同期 | [#173](https://github.com/kawaii-music-moe/yuuka/pull/173) | [yuuka-discord/src/tenant.rs](../../crates/yuuka-discord/src/tenant.rs)：稼働中のBotを1時間ごとに同期し、停止時にタイマーも終了。 |
| R21 | 修正済み | 一般ロールメンション | [#164](https://github.com/kawaii-music-moe/yuuka/pull/164) | [yuuka-discord/src/message_flow.rs](../../crates/yuuka-discord/src/message_flow.rs)：Botが保持する一般ロールへのメンションも本人宛として検出。 |
| R22 | 修正済み | ツール実績保存 | [#167](https://github.com/kawaii-music-moe/yuuka/pull/167) | [yuuka-orchestrator/src/tool_outcome.rs](../../crates/yuuka-orchestrator/src/tool_outcome.rs)：秘書・汎用のFCループにDB記録先を接続してtool_outcomesへ永続化。 |
| R23 | 修正済み | 実利用メトリクス | [#177](https://github.com/kawaii-music-moe/yuuka/pull/177) | [yuuka-services/src/metrics.rs](../../crates/yuuka-services/src/metrics.rs)：実処理から共有MetricsRegistryへ記録。計画・非同期予測はR06/R07で追加。 |
| R24 | 修正済み | Bot別支払い集計 | [#151](https://github.com/kawaii-music-moe/yuuka/pull/151) | [yuuka-services/src/report.rs](../../crates/yuuka-services/src/report.rs)：日報・週報の支払い予定をuser_idとbot_idの両方で限定。 |
| R25 | 修正済み | RSS登録時・通信時検査 | [#156](https://github.com/kawaii-music-moe/yuuka/pull/156)、[#202](https://github.com/kawaii-music-moe/yuuka/pull/202) | [yuuka-briefing/src/service.rs](../../crates/yuuka-briefing/src/service.rs)：通信時はDNS全件と各リダイレクト先を検査。登録時もRegistrationPolicy::PublicHttpでURL正規化後に共通の内部/予約IP範囲を拒否する（DNSは通信時だけ解決）。 |
| R26 | 修正済み | 運用設定 | [#176](https://github.com/kawaii-music-moe/yuuka/pull/176) | [yuuka-core/src/config.rs](../../crates/yuuka-core/src/config.rs)：REMINDER_CRON、SYNAPSE_*、デスクトップTTLを実処理へ反映。 |
| R27 | 修正済み | 所有権確認meta | [#166](https://github.com/kawaii-music-moe/yuuka/pull/166) | [yuuka-web/src/static_files.rs](../../crates/yuuka-web/src/static_files.rs)：管理画面/PWAのHTMLへエスケープ済みgoogle-site-verificationを挿入。 |

## D01〜D08

| ID | 状態・方針 | 実装PR | 根拠・残る課題 |
|---|---|---|---|
| D01 | 整理済み。管理画面とPWAは用途別に共存 | [#194](https://github.com/kawaii-music-moe/yuuka/pull/194) | [フロント契約監査](2026-10-20-frontend-contracts.md)。機能差を明示し、業務ルールはRust側へ置く。 |
| D02 | 整理済み。書き込みを共通サービスへ集約 | [#197](https://github.com/kawaii-music-moe/yuuka/pull/197) | [ドメイン監査](2026-10-20-domain-services.md)。入口の応答形式・入力差は維持。 |
| D03 | 修正済み。Google同期込みで予定サービスへ統一 | [#162](https://github.com/kawaii-music-moe/yuuka/pull/162) | `yuuka-schedule/src/service.rs`のScheduleServiceを登録・削除の全入口で使用。 |
| D04 | 修正済み。cronの登録・実行パーサを統一 | [#160](https://github.com/kawaii-music-moe/yuuka/pull/160) | `yuuka-core/src/cron_util.rs`。R19と同じ実装。 |
| D05 | 整理済み。日時処理を共通関数へ集約 | [#163](https://github.com/kawaii-music-moe/yuuka/pull/163) | `yuuka-core/src/datetime.rs`。ローカル時刻・JST・PWA向けUTCの用途差を保持。 |
| D06 | 共通化済み、登録時検査も修正済み | [#189](https://github.com/kawaii-music-moe/yuuka/pull/189)、[#202](https://github.com/kawaii-music-moe/yuuka/pull/202) | [外向き通信監査](2026-10-20-d06-ssrf-common.md)。yuuka-outboundへ集約。BriefingLegacyをRegistrationPolicy::PublicHttpへ置き換え、登録時もURL正規化後に通信時と同じ内部/予約IP範囲を拒否する。通信時のDNS全件・RSS各ホップの検査は維持。 |
| D07 | 整理済み。同じwire契約は生成型を利用 | [#194](https://github.com/kawaii-music-moe/yuuka/pull/194) | [フロント契約監査](2026-10-20-frontend-contracts.md)。PWAは別契約の変換・契約テストを保持。 |
| D08 | 整理済み。OCR抽出を共通化 | [#193](https://github.com/kawaii-music-moe/yuuka/pull/193) | [レシート監査](2026-10-20-d08-receipt-common.md)。PWAの下書きとDiscord/管理画面の秘書ターンの違いは維持。 |

## 仕様として受容した差と追加修正

- X01〜X03：操作履歴と制限カウンタはプロセス内メモリ、会話履歴はSQLite直読みを受容。[#172](https://github.com/kawaii-music-moe/yuuka/pull/172)のレシート制限共有に続き、[#203](https://github.com/kawaii-music-moe/yuuka/pull/203)でChatEngineがlimiterを保持し、Discord汎用ギルド・PWAチャット・PWA/管理画面レシートの全経路で共有する。判定はChatEngine::acquire_turnへ集約し、実行APIはTurnPermitを必須にした。PWAの保存前429、Discordのメンバー・キー確認後の判定、レシートの二重消費防止を維持する。Discord秘書・Bot DM・WSチャット・定期playbookの既存の免除範囲も維持する。メモリ保持等を受容した理由は元の棚卸しの「設計上の変更・縮小」を参照。
- X04：[#178](https://github.com/kawaii-music-moe/yuuka/pull/178)で`runBriefingNow(deliver: true)`の設定先配信を復旧。既定は会話内表示。
- X05：[#175](https://github.com/kawaii-music-moe/yuuka/pull/175)で定時・テスト配信のEmbedを復旧。
- SD07：[#188](https://github.com/kawaii-music-moe/yuuka/pull/188)でbot_idの暗黙既定値と退役MCP列を除去。[移行の検証範囲](2026-10-20-sd07-bot-id-default.md)。
- R24の補足：[#153](https://github.com/kawaii-music-moe/yuuka/pull/153)で繰り返し支払いの次回分にも元のbot_idを引き継ぐ。

## 追加課題の対応と検証範囲

| Issue | 対応PR（developへマージ済み） | 対応内容 |
|---|---|---|
| [#182](https://github.com/kawaii-music-moe/yuuka/issues/182) | [#200](https://github.com/kawaii-music-moe/yuuka/pull/200)（`2b9754c`） | 記帳後の補足取得失敗でも記帳成功と未取得項目を返し、再記帳不要と案内する。 |
| [#184](https://github.com/kawaii-music-moe/yuuka/issues/184) | [#202](https://github.com/kawaii-music-moe/yuuka/pull/202)（`97566f0`） | 登録時も共通のIP拒否範囲を使い、省略形・整数表記のIPv4を正規化して拒否する。通信時のDNS検査は維持する。 |
| [#192](https://github.com/kawaii-music-moe/yuuka/issues/192) | [#203](https://github.com/kawaii-music-moe/yuuka/pull/203)（`b048572`） | ChatEngineのpermitへ制限判定を集約し、入口の付け忘れ防止、PWAの保存前429、Discordの判定順を維持する。 |

上記3件は本表の基準時点で実装がマージ済みだが、IssueはOPENのまま。developへのマージとIssueの自動クローズは連動しないため、OPENであることを実装未完了の根拠にしない。

#97の完了判断では、本表のマージ済み修正と各PRに記載された未実施検証を確認する。#200 / #202 / #203の回帰テストは追加・型検査までで、作業ブランチでのcargo testは未実施。Rustテストはdevelop → mainのCIで実行するため、通常のdevelop向けPRのCI成功だけでテスト実行済みとは扱わない。実Discord/Gemini通信など各PRの未確認範囲も残る。保留中の#185は移行差分の対応範囲に含めない。

