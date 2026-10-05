//! 共通レスポンスエンベロープ（§10.2）。
//!
//! 既存フロント `ApiResponse<T> = { success: boolean; message?: string } & T` の
//! **トップレベル直置き**形状（`data` ラッパ不在）を Rust 側の単一真実源として表現する。
//! `#[serde(flatten)]` で payload をエンベロープと平坦化し、既存 JSON 形状を維持する。

use serde::Serialize;
use ts_rs::TS;

/// 共通エンベロープ。`data` はトップレベルへ flatten される（`{ success, message?, ...data }`）。
///
/// ts-rs では flatten されたジェネリック `T` は交差型 `{ success, message? } & T` として
/// 生成される（既存 `types.ts` の `ApiResponse<T>` と一致）。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct Envelope<T>
where
    T: TS,
{
    /// 成否フラグ。
    pub success: bool,
    /// 任意メッセージ（エラー時等）。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[ts(optional)]
    pub message: Option<String>,
    /// ペイロード本体（トップレベルへ flatten）。
    #[serde(flatten)]
    pub data: T,
}

/// 空ペイロード（`{ success }` のみを返す mutation 用）。Node の delete `{ success: <bool> }`
/// に一致させる。`#[serde(flatten)]` で無へ畳まれるため追加フィールドを生まない。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "generated/")]
pub struct EmptyData {}

impl Envelope<EmptyData> {
    /// `{ success }` のみのエンベロープ（削除可否など・Node parity）。
    #[must_use]
    pub fn bare(success: bool) -> Self {
        Self {
            success,
            message: None,
            data: EmptyData {},
        }
    }
}

impl<T> Envelope<T>
where
    T: TS,
{
    /// 成功エンベロープ。
    #[must_use]
    pub fn ok(data: T) -> Self {
        Self {
            success: true,
            message: None,
            data,
        }
    }

    /// メッセージ付き成功エンベロープ。
    #[must_use]
    pub fn ok_with_message(data: T, message: impl Into<String>) -> Self {
        Self {
            success: true,
            message: Some(message.into()),
            data,
        }
    }

    /// 失敗エンベロープ。
    #[must_use]
    pub fn err(data: T, message: impl Into<String>) -> Self {
        Self {
            success: false,
            message: Some(message.into()),
            data,
        }
    }
}
