//! WebSocket チャットプロトコルの serde 型定義。
//!
//! **このモジュールはバックエンドとの「契約」であり、唯一の正は
//! [`docs/design/desktop_client/backend_api.md`] §3 のワイヤ形である。**
//! 余計な依存を持たず自己完結させること。フレームは全て JSON テキスト
//! フレームで、`{"type":"..."}` タグ付き discriminated union。
//!
//! - クライアント → サーバ: [`ClientFrame`]（`msg` / `reset` / `ping`）
//! - サーバ → クライアント: [`ServerFrame`]
//!   （`ready` / `status` / `interim` / `token` / `done` / `push` / `error`）
//!
//! `botId` は WS 接続時に `?botId=` で束縛されるため、各メッセージには含めない
//! （backend_api.md §3.2）。

use serde::{Deserialize, Serialize};

// ===========================================================================
// 添付（画像 / 音声）— base64 を JSON に内包（backend_api.md §3.2 / §3.3）
// ===========================================================================

/// 画像/音声の添付ペイロード。`data` は base64 エンコード済み文字列。
///
/// 送信（`msg`）の image/audio と、受信（`done`/`push`）の `files[]` 形が
/// それぞれ別物（後者は `name` を持つ）なので、用途別に型を分けている。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Attachment {
    /// MIME タイプ。例: 画像 `"image/png"`、音声 `"audio/ogg"`。
    pub mime: String,
    /// base64 エンコードされたバイト列。
    pub data: String,
}

// ===========================================================================
// クライアント → サーバ（backend_api.md §3.2）
// ===========================================================================

/// クライアントが WS で送るフレーム。
///
/// `#[serde(tag = "type", rename_all = "lowercase")]` でドキュメントの
/// `{"type":"msg"|"reset"|"ping"}` 形に一致させる。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ClientFrame {
    /// 発話。テキスト＋任意の画像/音声＋任意の返信チェーン ID。
    Msg {
        /// 本文（空文字許容。画像/音声のみの発話もありうる）。
        text: String,
        /// 任意の画像添付（`{mime, data}`）。
        #[serde(skip_serializing_if = "Option::is_none", default)]
        image: Option<Attachment>,
        /// 任意の音声添付（`{mime, data}`、推奨 `audio/ogg`）。
        #[serde(skip_serializing_if = "Option::is_none", default)]
        audio: Option<Attachment>,
        /// 任意: 返信チェーンの対象メッセージ ID。
        #[serde(rename = "replyToId", skip_serializing_if = "Option::is_none", default)]
        reply_to_id: Option<String>,
    },

    /// 会話リセット（接続束縛の Bot に対するコンテキストクリア）。
    Reset,

    /// keepalive（任意。WS ping/pong を使うなら不要だがプロトコルには存在）。
    Ping,

    /// ボタン押下（コンポーネント・インタラクション。ws_components.md §3）。
    /// `{"type":"interaction","messageId":"...","customId":"..."}`。
    Interaction {
        #[serde(rename = "messageId")]
        message_id: String,
        #[serde(rename = "customId")]
        custom_id: String,
    },
}

impl ClientFrame {
    /// テキストのみの発話を作るショートカット。
    pub fn text(text: impl Into<String>) -> Self {
        ClientFrame::Msg {
            text: text.into(),
            image: None,
            audio: None,
            reply_to_id: None,
        }
    }
}

// ===========================================================================
// リッチコンテンツ（embeds / files）— backend_api.md §3.3 / architecture.md §7
// ===========================================================================

/// Embed のフィールド（`{name, value, inline?}`）。
///
/// discord.js の `EmbedBuilder#fields` を中立 JSON に直列化したもの。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct EmbedField {
    pub name: String,
    pub value: String,
    /// 横並び表示の可否。Discord 互換のため任意。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub inline: Option<bool>,
}

/// Embed 内の画像参照。`name` は同梱 `files[]` の同名添付へ解決してインライン表示する
/// （architecture.md §7: `attachment://chart.png` → base64 添付）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmbedImage {
    pub name: String,
}

/// Embed カード（タイトル / 本文 / 色帯 / フィールド / 画像）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Embed {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description: Option<String>,
    /// 色帯（10 進 RGB 整数。例 `3447003`）。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub color: Option<u32>,
    /// フィールド配列。省略時は空。
    #[serde(default)]
    pub fields: Vec<EmbedField>,
    /// インライン画像参照。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub image: Option<EmbedImage>,
}

// ===========================================================================
// 対話コンポーネント（action row / button）— ws_components.md §1
// ===========================================================================
//
// Discord API JSON（`.toJSON()` 出力）をそのまま運ぶ。`type` 数値で分岐し、
// 未知の `type`（セレクト等）は受信で落ちないよう `Component::Unknown` へ握りつぶす。

/// アクション行（`{type:1, components:[...]}`）。中の `components` がボタン列。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ActionRow {
    #[serde(default)]
    pub components: Vec<Component>,
}

/// 行内コンポーネント。Discord の数値 `type` で分岐する
/// （1=row, 2=button, それ以外=Unknown でスキップ）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Component {
    /// ネストした action row（`type:1`）。
    Row(ActionRow),
    /// ボタン（`type:2`）。
    Button {
        /// 1=Primary 2=Secondary 3=Success 4=Danger 5=Link。
        style: u8,
        label: Option<String>,
        /// Discord JSON は snake_case の `custom_id`。Link ボタンには無い。
        custom_id: Option<String>,
        /// Link（style 5）ボタンの遷移先 URL。
        url: Option<String>,
        /// 非活性フラグ。省略時 false。
        disabled: bool,
    },
    /// 未知の `type`（セレクト等）。受信で落とさないため保持だけする（描画しない）。
    Unknown,
}

// Component は Discord の数値タグ `type` で内容が変わるため、中間表現を経由して
// 手書きの serde を実装する（`#[serde(tag=...)]` は数値タグを取れないため）。
#[derive(Deserialize)]
struct ComponentRepr {
    #[serde(rename = "type")]
    kind: u8,
    #[serde(default)]
    components: Vec<Component>,
    #[serde(default)]
    style: u8,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    custom_id: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    disabled: bool,
}

impl<'de> Deserialize<'de> for Component {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let r = ComponentRepr::deserialize(deserializer)?;
        Ok(match r.kind {
            1 => Component::Row(ActionRow {
                components: r.components,
            }),
            2 => Component::Button {
                style: r.style,
                label: r.label,
                custom_id: r.custom_id,
                url: r.url,
                disabled: r.disabled,
            },
            _ => Component::Unknown,
        })
    }
}

impl Serialize for Component {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        match self {
            Component::Row(row) => {
                let mut m = serializer.serialize_map(Some(2))?;
                m.serialize_entry("type", &1u8)?;
                m.serialize_entry("components", &row.components)?;
                m.end()
            }
            Component::Button {
                style,
                label,
                custom_id,
                url,
                disabled,
            } => {
                let mut m = serializer.serialize_map(None)?;
                m.serialize_entry("type", &2u8)?;
                m.serialize_entry("style", style)?;
                if let Some(label) = label {
                    m.serialize_entry("label", label)?;
                }
                if let Some(custom_id) = custom_id {
                    m.serialize_entry("custom_id", custom_id)?;
                }
                if let Some(url) = url {
                    m.serialize_entry("url", url)?;
                }
                if *disabled {
                    m.serialize_entry("disabled", disabled)?;
                }
                m.end()
            }
            // Unknown は元の型情報を保持していないため出力できない。受信専用。
            Component::Unknown => {
                let mut m = serializer.serialize_map(Some(1))?;
                m.serialize_entry("type", &0u8)?;
                m.end()
            }
        }
    }
}

/// 受信ファイル（チャート PNG 等）。`data` は base64。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FilePayload {
    pub name: String,
    pub mime: String,
    /// base64 エンコードされたバイト列。
    pub data: String,
}

// ===========================================================================
// `ready` フレームの構造体（backend_api.md §3.1 / §3.3）
// ===========================================================================

/// 認証済みユーザー情報（`ready.user`）。
///
/// バックエンドの `SessionUser` 相当。フィールド名はサーバ実装に合わせる
/// （Discord ID は `discordId`、ロールは任意）。前方互換のため未知フィールドは無視。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct UserInfo {
    #[serde(rename = "discordId")]
    pub discord_id: String,
    #[serde(default)]
    pub username: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub role: Option<String>,
}

/// Bot のメタ情報（`ready.bot` / `ready.bots[]`）。
///
/// オーブのアイコン（`discord_avatar_url`）と、ホットキー登録の可否
/// （`primary`）に使う。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct BotInfo {
    pub id: String,
    /// 表示名。欠落しても 1 件の不正 Bot で `ready` 全体のデコードが落ち、セレクタが
    /// 空（＝何も選べない）にならないよう、未指定は空文字に倒す（既定許容）。
    #[serde(default)]
    pub name: String,
    /// オーブに描く Discord アバター URL。未設定の Bot もありうるので任意。
    #[serde(
        rename = "discord_avatar_url",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub discord_avatar_url: Option<String>,
    /// プライマリ Bot か（グローバルホットキー登録の可否判定）。
    #[serde(default)]
    pub primary: bool,
}

// ===========================================================================
// サーバ → クライアント（backend_api.md §3.3）
// ===========================================================================

/// サーバ進捗ステータス（`status.state`）。
///
/// `onStatusChange("thinking"|"writing")` を直列化したもの（`idle` は送らない）。
/// 前方互換のため未知の値も保持できるよう `Other` を用意する。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StatusState {
    Thinking,
    Writing,
    /// 未知のステータス（プロトコル拡張に前方互換で耐える）。
    #[serde(other)]
    Other,
}

/// サーバが WS で送るフレーム。
///
/// `#[serde(tag = "type", rename_all = "lowercase")]` でドキュメントの
/// `{"type":"ready"|"status"|...}` 形に一致させる。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ServerFrame {
    /// 接続確立直後に一度だけ届く。オーブ画像・切替セレクタ・送信上限を確定する。
    Ready {
        user: UserInfo,
        /// この接続が束縛された Bot。
        bot: BotInfo,
        /// 利用可能な Bot 一覧（切替 UI / 別オーバーレイ導線）。
        #[serde(default)]
        bots: Vec<BotInfo>,
        /// 1 メッセージ添付上限（MB）。送信前にローカルで弾く。
        #[serde(rename = "maxUploadMb")]
        max_upload_mb: u32,
    },

    /// 進捗ステータス（「考え中…」「入力中…」）。
    Status { state: StatusState },

    /// 重い処理の一時応答（通常の assistant 気泡として差し込む）。
    Interim { text: String },

    /// 将来: トークン逐次（v1 では送られない。architecture.md §6）。
    Token { delta: String },

    /// ターン最終応答（テキスト＋リッチコンテンツ）。
    Done {
        #[serde(rename = "messageId")]
        message_id: String,
        text: String,
        #[serde(default)]
        embeds: Vec<Embed>,
        #[serde(default)]
        files: Vec<FilePayload>,
        /// 任意の対話コンポーネント（action row）。省略時は空（ws_components.md §2）。
        #[serde(default)]
        components: Vec<ActionRow>,
        /// `true` の場合、後続で `push` フレームが届く（重い処理）。
        #[serde(default)]
        deferred: bool,
    },

    /// 重い処理（`done` で `deferred:true` を返した後）の最終結果。
    Push {
        /// components を載せる場合の突合キー（ws_components.md §2）。
        #[serde(rename = "messageId", default)]
        message_id: Option<String>,
        text: String,
        #[serde(default)]
        embeds: Vec<Embed>,
        #[serde(default)]
        files: Vec<FilePayload>,
        /// 任意の対話コンポーネント（action row）。
        #[serde(default)]
        components: Vec<ActionRow>,
    },

    /// 既存メッセージの書き換え（interaction の結果。ws_components.md §2）。
    /// `messageId` で履歴中の該当メッセージを探し、存在するフィールドのみ差し替える。
    Update {
        #[serde(rename = "messageId")]
        message_id: String,
        #[serde(default)]
        text: Option<String>,
        /// `Some(vec![])` でボタン除去、`None` で components 据え置き。
        #[serde(default)]
        components: Option<Vec<ActionRow>>,
        #[serde(default)]
        embeds: Option<Vec<Embed>>,
    },

    /// エラー（コード＋人間向けメッセージ）。
    Error { code: ErrorCode, message: String },
}

/// エラーコード（backend_api.md §7）。未知コードにも前方互換で耐える。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// トークン無効/失効 → 保存トークン破棄して再ログイン。
    Unauthorized,
    /// ユーザーの Gemini キー未設定 → Web 設定へ誘導。
    NoGeminiKey,
    /// レート超過 → バックオフして再送。
    RateLimited,
    /// ペイロード超過 → 画像縮小/音声短縮を促す。
    TooLarge,
    /// サーバ内部エラー → 再試行を促す。
    Internal,
    /// 未知のコード（プロトコル拡張に前方互換で耐える）。
    #[serde(other)]
    Unknown,
}

// ===========================================================================
// テスト: serde 往復（model.rs は契約なので最重要。設計 §12）
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_msg_roundtrip_minimal() {
        let frame = ClientFrame::text("こんにちは");
        let json = serde_json::to_string(&frame).unwrap();
        // image/audio/replyToId は None なので出力されないこと。
        assert_eq!(json, r#"{"type":"msg","text":"こんにちは"}"#);
        let back: ClientFrame = serde_json::from_str(&json).unwrap();
        assert_eq!(frame, back);
    }

    #[test]
    fn client_msg_with_attachments() {
        let frame = ClientFrame::Msg {
            text: "これ何のエラー？".into(),
            image: Some(Attachment {
                mime: "image/png".into(),
                data: "QUJD".into(),
            }),
            audio: None,
            reply_to_id: Some("123".into()),
        };
        let json = serde_json::to_string(&frame).unwrap();
        let back: ClientFrame = serde_json::from_str(&json).unwrap();
        assert_eq!(frame, back);
        // ドキュメントの replyToId キー名であること。
        assert!(json.contains("\"replyToId\":\"123\""));
        assert!(json.contains("\"image\":{\"mime\":\"image/png\",\"data\":\"QUJD\"}"));
    }

    #[test]
    fn client_reset_and_ping_shape() {
        assert_eq!(
            serde_json::to_string(&ClientFrame::Reset).unwrap(),
            r#"{"type":"reset"}"#
        );
        assert_eq!(
            serde_json::to_string(&ClientFrame::Ping).unwrap(),
            r#"{"type":"ping"}"#
        );
    }

    #[test]
    fn server_ready_parse() {
        let json = r#"{
            "type":"ready",
            "user":{"discordId":"u1","username":"testuser","role":"admin"},
            "bot":{"id":"b1","name":"ユウカ","discord_avatar_url":"https://x/a.png","primary":true},
            "bots":[{"id":"b1","name":"ユウカ","discord_avatar_url":"https://x/a.png","primary":true},
                    {"id":"b2","name":"カヨコ","primary":false}],
            "maxUploadMb":20
        }"#;
        let frame: ServerFrame = serde_json::from_str(json).unwrap();
        match frame {
            ServerFrame::Ready {
                bot,
                bots,
                max_upload_mb,
                user,
            } => {
                assert_eq!(user.discord_id, "u1");
                assert_eq!(bot.id, "b1");
                assert!(bot.primary);
                assert_eq!(bots.len(), 2);
                // discord_avatar_url 未設定の Bot も許容される。
                assert!(bots[1].discord_avatar_url.is_none());
                assert_eq!(max_upload_mb, 20);
            }
            _ => panic!("expected ready"),
        }
    }

    #[test]
    fn server_ready_tolerates_bot_missing_name() {
        // 1 件の Bot が name を欠いても ready 全体のデコードは落ちず、欠落名は空文字へ。
        // （こうしないとセレクタが空になり「個人用 Bot が選べない」事故になりうる。）
        let json = r#"{
            "type":"ready",
            "user":{"discordId":"u1"},
            "bot":{"id":"b1","name":"ユウカ","primary":true},
            "bots":[{"id":"b1","name":"ユウカ","primary":true},
                    {"id":"b2"}],
            "maxUploadMb":20
        }"#;
        let frame: ServerFrame = serde_json::from_str(json).unwrap();
        match frame {
            ServerFrame::Ready { bots, .. } => {
                assert_eq!(bots.len(), 2);
                assert_eq!(bots[1].id, "b2");
                assert_eq!(bots[1].name, "");
            }
            _ => panic!("expected ready"),
        }
    }

    #[test]
    fn server_status_parse() {
        let f: ServerFrame =
            serde_json::from_str(r#"{"type":"status","state":"thinking"}"#).unwrap();
        assert_eq!(
            f,
            ServerFrame::Status {
                state: StatusState::Thinking
            }
        );
        // 未知の state も Other に落ちる（前方互換）。
        let f2: ServerFrame =
            serde_json::from_str(r#"{"type":"status","state":"reflecting"}"#).unwrap();
        assert_eq!(
            f2,
            ServerFrame::Status {
                state: StatusState::Other
            }
        );
    }

    #[test]
    fn server_done_with_rich() {
        let json = r#"{
            "type":"done",
            "messageId":"m1",
            "text":"リマインドを登録しました。",
            "embeds":[{"title":"予定","description":"歯医者","color":3447003,
                       "fields":[{"name":"日時","value":"明日10時","inline":true}],
                       "image":{"name":"chart.png"}}],
            "files":[{"name":"chart.png","mime":"image/png","data":"QUJD"}],
            "deferred":false
        }"#;
        let frame: ServerFrame = serde_json::from_str(json).unwrap();
        match frame {
            ServerFrame::Done {
                message_id,
                embeds,
                files,
                deferred,
                ..
            } => {
                assert_eq!(message_id, "m1");
                assert_eq!(embeds.len(), 1);
                assert_eq!(embeds[0].color, Some(3447003));
                assert_eq!(embeds[0].fields[0].inline, Some(true));
                assert_eq!(embeds[0].image.as_ref().unwrap().name, "chart.png");
                assert_eq!(files[0].mime, "image/png");
                assert!(!deferred);
            }
            _ => panic!("expected done"),
        }
    }

    #[test]
    fn server_done_minimal_text_only() {
        // richReplyEnabled=false のユーザーは embeds/files が無い（テキストのみ）。
        let json = r#"{"type":"done","messageId":"m2","text":"はい。"}"#;
        let frame: ServerFrame = serde_json::from_str(json).unwrap();
        match frame {
            ServerFrame::Done {
                embeds,
                files,
                deferred,
                ..
            } => {
                assert!(embeds.is_empty());
                assert!(files.is_empty());
                assert!(!deferred);
            }
            _ => panic!("expected done"),
        }
    }

    #[test]
    fn server_push_and_interim() {
        let i: ServerFrame =
            serde_json::from_str(r#"{"type":"interim","text":"やっておきます。"}"#).unwrap();
        assert_eq!(
            i,
            ServerFrame::Interim {
                text: "やっておきます。".into()
            }
        );

        let p: ServerFrame = serde_json::from_str(
            r#"{"type":"push","text":"完了しました。","embeds":[],"files":[]}"#,
        )
        .unwrap();
        match p {
            ServerFrame::Push { text, .. } => assert_eq!(text, "完了しました。"),
            _ => panic!("expected push"),
        }
    }

    #[test]
    fn server_done_with_components() {
        // done に action row（Primary ボタン）が載るケース（ws_components.md §1-2）。
        let json = r#"{
            "type":"done","messageId":"m3","text":"確認してください。",
            "components":[{"type":1,"components":[
                {"type":2,"style":1,"label":"確認","custom_id":"demo_echo:abc"},
                {"type":2,"style":5,"label":"開く","url":"https://x/y"},
                {"type":3,"custom_id":"select:ignored"}
            ]}]
        }"#;
        let frame: ServerFrame = serde_json::from_str(json).unwrap();
        match frame {
            ServerFrame::Done {
                message_id,
                components,
                ..
            } => {
                assert_eq!(message_id, "m3");
                assert_eq!(components.len(), 1);
                let comps = &components[0].components;
                // 未知 type:3 は Unknown へ握りつぶされる（落ちない）。
                assert_eq!(comps.len(), 3);
                assert_eq!(
                    comps[0],
                    Component::Button {
                        style: 1,
                        label: Some("確認".into()),
                        custom_id: Some("demo_echo:abc".into()),
                        url: None,
                        disabled: false,
                    }
                );
                assert!(matches!(
                    comps[1],
                    Component::Button {
                        style: 5,
                        url: Some(_),
                        ..
                    }
                ));
                assert_eq!(comps[2], Component::Unknown);
            }
            _ => panic!("expected done"),
        }
    }

    #[test]
    fn server_update_parse() {
        // update フレーム: text 差し替え + components:[] でボタン除去。
        let json = r#"{"type":"update","messageId":"m3","text":"受け取りました","components":[]}"#;
        let frame: ServerFrame = serde_json::from_str(json).unwrap();
        match frame {
            ServerFrame::Update {
                message_id,
                text,
                components,
                embeds,
            } => {
                assert_eq!(message_id, "m3");
                assert_eq!(text.as_deref(), Some("受け取りました"));
                assert_eq!(components, Some(vec![]));
                assert!(embeds.is_none());
            }
            _ => panic!("expected update"),
        }

        // フィールド省略は据え置き（None）として解釈される。
        let f2: ServerFrame =
            serde_json::from_str(r#"{"type":"update","messageId":"m4"}"#).unwrap();
        match f2 {
            ServerFrame::Update {
                text, components, ..
            } => {
                assert!(text.is_none());
                assert!(components.is_none());
            }
            _ => panic!("expected update"),
        }
    }

    #[test]
    fn client_interaction_serialize() {
        let frame = ClientFrame::Interaction {
            message_id: "m3".into(),
            custom_id: "demo_echo:abc".into(),
        };
        let json = serde_json::to_string(&frame).unwrap();
        assert_eq!(
            json,
            r#"{"type":"interaction","messageId":"m3","customId":"demo_echo:abc"}"#
        );
        let back: ClientFrame = serde_json::from_str(&json).unwrap();
        assert_eq!(frame, back);
    }

    #[test]
    fn server_error_codes() {
        let cases = [
            (
                r#"{"type":"error","code":"unauthorized","message":"x"}"#,
                ErrorCode::Unauthorized,
            ),
            (
                r#"{"type":"error","code":"no_gemini_key","message":"x"}"#,
                ErrorCode::NoGeminiKey,
            ),
            (
                r#"{"type":"error","code":"rate_limited","message":"x"}"#,
                ErrorCode::RateLimited,
            ),
            (
                r#"{"type":"error","code":"too_large","message":"x"}"#,
                ErrorCode::TooLarge,
            ),
            (
                r#"{"type":"error","code":"internal","message":"x"}"#,
                ErrorCode::Internal,
            ),
            // 未知コードは Unknown へ（前方互換）。
            (
                r#"{"type":"error","code":"teapot","message":"x"}"#,
                ErrorCode::Unknown,
            ),
        ];
        for (json, expected) in cases {
            match serde_json::from_str::<ServerFrame>(json).unwrap() {
                ServerFrame::Error { code, .. } => assert_eq!(code, expected),
                _ => panic!("expected error"),
            }
        }
    }
}
