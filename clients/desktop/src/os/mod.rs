//! OS 依存の薄い抽象（trait + プラットフォーム実装）。
//!
//! 仕様: client_design.md §4.2 / §4.4 / NFR-7（移植性）。
//! OS 固有処理（トレイ・ホットキー・資格情報ストア・オーバーレイ補完・単一
//! インスタンスロック）を trait の裏に隔離し、UI からは trait 越しにのみ触る。
//!
//! 現状は Windows 実装（`os::windows`）と、それ以外向けの no-op フォールバックを
//! 用意し、**全プラットフォームでコンパイルできる**ようにする
//! （Linux 上でのビルド検証も通す）。

use std::path::Path;

/// 単一インスタンスロックの取得結果。
pub enum InstanceLock {
    /// このプロセスが当該 botId の唯一のインスタンス（ロック取得成功）。
    /// ガードを保持し続ける（drop でロック解放）。
    Acquired(Box<dyn SingleInstanceGuard>),
    /// 既に同 botId のインスタンスが起動済み（自プロセスは終了すべき）。
    AlreadyRunning,
}

/// 単一インスタンスロックの保持ガード。drop でロックを解放する。
pub trait SingleInstanceGuard: Send {}

/// OS 統合の抽象インターフェース。
///
/// UI/Net 層はこの trait 越しにのみ OS 機能を使う。Windows 以外では各メソッドは
/// 安全な no-op か未対応エラーを返す（パニックしない）。
pub trait OsIntegration {
    /// botId をキーにした単一インスタンスロックを取得する（client_design.md §4.4）。
    ///
    /// `Local\yuuka-desktop-{botId}` 相当の名前付き Mutex（Windows）。
    fn acquire_single_instance(&self, bot_id: &str) -> InstanceLock;

    /// 自動起動（スタートアップ）登録の有効/無効を切り替える。
    ///
    /// `--bot <id>` 付きで登録する（常駐させたい Bot ごと。client_design.md §4.3）。
    /// 対応しないプラットフォームでは `Err`。
    fn set_autostart(&self, _enabled: bool, _bot_id: &str) -> Result<(), OsError> {
        Err(OsError::Unsupported("autostart"))
    }

    /// オーバーレイウィンドウのクリック透過/最前面を OS API で補完する
    /// （eframe の `mouse_passthrough` で不足する場合のみ。client_design.md §4.2）。
    ///
    /// `window_handle` はプラットフォーム固有のウィンドウハンドル（Windows なら HWND）。
    /// `passthrough=true` でクリック透過（WS_EX_LAYERED | WS_EX_TRANSPARENT）。
    fn set_overlay_passthrough(&self, _window_handle: isize, _passthrough: bool) {
        // 既定: 何もしない（eframe 標準で足りる前提）。
    }

    /// グローバルなカーソル位置（**物理スクリーンピクセル**・仮想デスクトップ原点）。
    ///
    /// オーバーレイのクリック透過を「オーブの上だけ無効化」するために使う
    /// （client_design.md §4.2）。クリック透過中は egui がマウス座標を受け取れない
    /// ため、OS へ直接問い合わせてオーブ矩形との内外を判定する。未対応の
    /// プラットフォームでは `None`（呼び出し側は安全側＝透過 OFF にフォールバックする）。
    fn cursor_pos_physical(&self) -> Option<(f32, f32)> {
        None
    }
}

/// OS 統合のエラー。
#[derive(Debug, thiserror::Error)]
pub enum OsError {
    #[error("operation not supported on this platform: {0}")]
    Unsupported(&'static str),
    #[error("os error: {0}")]
    Platform(String),
}

// ===========================================================================
// プラットフォーム選択
// ===========================================================================

#[cfg(windows)]
pub mod windows;

#[cfg(windows)]
pub use windows::WindowsOs as PlatformOs;

// 非 Windows 向け no-op フォールバックはこのファイル末尾にインライン定義する。
#[cfg(not(windows))]
pub use fallback::FallbackOs as PlatformOs;

/// 現在のプラットフォーム向け [`OsIntegration`] 実装を生成する。
pub fn platform() -> PlatformOs {
    PlatformOs::new()
}

// ===========================================================================
// 非 Windows 向け no-op フォールバック（Linux でのビルド検証用）
// ===========================================================================

#[cfg(not(windows))]
mod fallback {
    use super::*;

    /// 非 Windows 向けの安全な no-op 実装。
    ///
    /// 単一インスタンスロックはファイルロック等で実装可能だが、現フェーズでは
    /// 常に `Acquired`（抑止しない）とする。autostart/overlay は未対応。
    pub struct FallbackOs;

    impl FallbackOs {
        pub fn new() -> Self {
            FallbackOs
        }
    }

    struct NoopGuard;
    impl SingleInstanceGuard for NoopGuard {}

    impl OsIntegration for FallbackOs {
        fn acquire_single_instance(&self, _bot_id: &str) -> InstanceLock {
            // 非 Windows では二重起動抑止を行わない（将来 mac/Linux で実装）。
            InstanceLock::Acquired(Box::new(NoopGuard))
        }
    }
}

// ===========================================================================
// クロスプラットフォームなパスユーティリティ（自動起動の引数組み立て等で共有）
// ===========================================================================

/// 現在の実行ファイルパスを取得する（自動起動コマンド組み立て用）。
pub fn current_exe() -> std::io::Result<Box<Path>> {
    Ok(std::env::current_exe()?.into_boxed_path())
}
