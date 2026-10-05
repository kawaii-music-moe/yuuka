//! Windows 固有の OS 統合実装（`#[cfg(windows)]`）。
//!
//! 仕様: client_design.md §4.2 / §4.3 / §4.4。
//!
//! - **単一インスタンスロック**: botId をキーにした名前付き Mutex
//!   `Local\yuuka-desktop-{botId}`（CreateMutexW + ERROR_ALREADY_EXISTS 判定）。
//! - **自動起動**: レジストリ `HKCU\...\Run` に `--bot <id>` 付きで登録。
//! - **オーバーレイ補完**: WS_EX_LAYERED | WS_EX_TRANSPARENT でクリック透過、
//!   SetWindowPos(HWND_TOPMOST) で堅牢な最前面（eframe で不足する場合のみ）。
//!
//! 注: 本ファイルは Windows ターゲットでのみコンパイルされる。Linux 上の
//! `cargo check` では評価されない（プラットフォームゲートで除外）。

use windows::core::HSTRING;
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HWND, POINT,
};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, HWND_TOPMOST,
    SWP_NOMOVE, SWP_NOSIZE, WS_EX_LAYERED, WS_EX_TRANSPARENT,
};

use super::{InstanceLock, OsError, OsIntegration, SingleInstanceGuard};

/// Windows 向け OS 統合実装。
pub struct WindowsOs;

impl WindowsOs {
    pub fn new() -> Self {
        WindowsOs
    }
}

/// 名前付き Mutex を保持するガード。drop でハンドルを閉じてロックを解放する。
struct MutexGuard {
    handle: HANDLE,
}

// HANDLE は OS リソースで、別スレッドへ送っても安全（close だけ行う）。
unsafe impl Send for MutexGuard {}

impl SingleInstanceGuard for MutexGuard {}

impl Drop for MutexGuard {
    fn drop(&mut self) {
        // SAFETY: CreateMutexW で得た有効なハンドルを 1 度だけ閉じる。
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

impl OsIntegration for WindowsOs {
    fn acquire_single_instance(&self, bot_id: &str) -> InstanceLock {
        // botId キーの名前付き Mutex（同一 botId の二重起動を抑止。§4.4）。
        // セッションローカルで十分（per-user 常駐）。
        let name = HSTRING::from(format!("Local\\yuuka-desktop-{bot_id}"));

        // SAFETY: 名前付き Mutex を作成。既存なら ERROR_ALREADY_EXISTS が立つ。
        unsafe {
            match CreateMutexW(None, true, &name) {
                Ok(handle) => {
                    if GetLastError() == ERROR_ALREADY_EXISTS {
                        // 既に同 botId のインスタンスが Mutex を保持している。
                        let _ = CloseHandle(handle);
                        InstanceLock::AlreadyRunning
                    } else {
                        InstanceLock::Acquired(Box::new(MutexGuard { handle }))
                    }
                }
                Err(_) => {
                    // 生成失敗時は安全側（抑止しない）に倒し、起動を許す。
                    // ガード代わりに無効ハンドルは持たせない。
                    InstanceLock::Acquired(Box::new(NoopGuard))
                }
            }
        }
    }

    fn set_autostart(&self, enabled: bool, bot_id: &str) -> Result<(), OsError> {
        // レジストリ `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` への
        // 登録/削除。windows-rs の Registry API を使う実装は Phase 5 で完成させる。
        // ここではインターフェースのみ確定させ、未実装を明示する。
        let _ = (enabled, bot_id);
        Err(OsError::Unsupported("autostart (TODO: Phase 5)"))
    }

    fn set_overlay_passthrough(&self, window_handle: isize, passthrough: bool) {
        if window_handle == 0 {
            return;
        }
        let hwnd = HWND(window_handle as *mut _);
        // SAFETY: 有効な HWND に対する拡張スタイルの読み書きと再配置。
        unsafe {
            let mut ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let bits = (WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0) as isize;
            if passthrough {
                ex_style |= bits; // クリック透過を有効化。
            } else {
                ex_style &= !bits; // モーダル展開時は透過解除。
            }
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex_style);

            // 堅牢な最前面（位置/サイズは保持）。
            let _ = SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
        }
    }

    fn cursor_pos_physical(&self) -> Option<(f32, f32)> {
        let mut pt = POINT { x: 0, y: 0 };
        // SAFETY: pt は有効なスタック変数。GetCursorPos は成功時に物理スクリーン座標
        // （仮想デスクトップ原点）を書き込む。失敗（稀）なら None。
        unsafe { GetCursorPos(&mut pt) }.ok()?;
        Some((pt.x as f32, pt.y as f32))
    }
}

/// Mutex 生成に失敗した場合のダミーガード（抑止しない安全側フォールバック）。
struct NoopGuard;
impl SingleInstanceGuard for NoopGuard {}
