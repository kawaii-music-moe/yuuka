//! 会話単位のターン直列化（`bot × channel` の keyed async mutex）。
//!
//! 全 `MessageCreate` は tokio::spawn で並行処理されるが、ターン処理は
//! **persist-before-load**（発言を履歴へ永続化してから履歴を読む）のため、同一チャンネルで
//! 同時進行するターン同士が互いの「未応答の発言」を履歴末尾に拾い、双方の返信が両方の話題へ
//! 答えて混ざる（返信の交錯）。本ゲートで同一会話（bot × channel）のターンを到着順に直列化し、
//! 各ターンが「前のターンの返信まで確定した履歴」を見ることを保証する。
//!
//! 異なるチャンネル・異なる Bot のターンは従来どおり並行に処理される。

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, PoisonError};

use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

/// キー（`{bot_id}:{channel_id}`）ごとの非同期ロック表。
///
/// エントリは獲得時に日和見掃除する（誰も保持/待機していない = `Arc` が表の 1 本だけの
/// エントリを落とす）ため、チャンネル数に比例して無限成長しない。
#[derive(Default)]
pub struct TurnGate {
    locks: StdMutex<HashMap<String, Arc<AsyncMutex<()>>>>,
}

impl TurnGate {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `key` のロックを獲得する（解放は返り値の drop）。同一 key は獲得要求順に直列化される。
    pub async fn acquire(&self, key: &str) -> OwnedMutexGuard<()> {
        let lock = {
            let mut map = self.locks.lock().unwrap_or_else(PoisonError::into_inner);
            // 日和見掃除: 保持者も待機者もいない（表の参照のみ）エントリを除去。
            map.retain(|_, m| Arc::strong_count(m) > 1);
            map.entry(key.to_owned()).or_default().clone()
        };
        lock.lock_owned().await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[tokio::test]
    async fn same_key_serializes_in_order() {
        let gate = Arc::new(TurnGate::new());
        let running = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..8 {
            let (gate, running, max_seen) = (gate.clone(), running.clone(), max_seen.clone());
            handles.push(tokio::spawn(async move {
                let _guard = gate.acquire("bot:chan").await;
                let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                max_seen.fetch_max(now, Ordering::SeqCst);
                tokio::task::yield_now().await;
                running.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for h in handles {
            h.await.expect("join");
        }
        // 同一 key は同時実行 1 に抑えられる。
        assert_eq!(max_seen.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn different_keys_run_concurrently() {
        let gate = Arc::new(TurnGate::new());
        let g1 = gate.acquire("bot:chan-a").await;
        // 別 key は g1 保持中でも即時獲得できる（デッドロックしない）。
        let g2 = gate.acquire("bot:chan-b").await;
        drop(g1);
        drop(g2);
    }

    #[tokio::test]
    async fn idle_entries_are_swept() {
        let gate = TurnGate::new();
        drop(gate.acquire("bot:chan-a").await);
        drop(gate.acquire("bot:chan-b").await);
        // 次の獲得で未使用エントリが掃除される（自 key は獲得中のため残る）。
        let _g = gate.acquire("bot:chan-c").await;
        let map = gate.locks.lock().expect("lock");
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("bot:chan-c"));
    }
}
