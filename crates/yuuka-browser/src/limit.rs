//! Chromium の同時起動数の上限（環境変数 `YUUKA_BROWSER_MAX_PROCESSES`）。
//!
//! 対話セッションは開いている間ずっと、1 回きりの起動（`--dump-dom`/`--screenshot`）は終わるまで、
//! それぞれ 1 枠を使う。未設定・空・`0` なら上限なし。不正値は警告して上限なしにする。
//! 枠が埋まっているときは [`WAIT`] まで空きを待ち、それでも空かなければエラーを返す。

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const ENV: &str = "YUUKA_BROWSER_MAX_PROCESSES";
/// 枠の空きを待つ時間。
const WAIT: Duration = Duration::from_secs(30);
const BUSY: &str =
    "ブラウザの同時起動数の上限に達しています。しばらく待ってからもう一度お試しください。";

/// 使用中の 1 枠。落とすと空く（上限なしのときは何も持たない）。
pub(crate) struct Slot {
    _permit: Option<OwnedSemaphorePermit>,
}

/// 環境変数の値を上限に直す（`None` = 上限なし）。
fn parse_limit(raw: Option<&str>) -> Option<usize> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    match raw.parse::<usize>() {
        Ok(0) => None,
        Ok(n) => Some(n.min(Semaphore::MAX_PERMITS)),
        Err(_) => {
            tracing::warn!(
                value = raw,
                "{ENV} が不正なため、ブラウザの同時起動数を制限しません"
            );
            None
        }
    }
}

/// プロセス全体で共有する枠（初回に環境変数を読む）。
fn semaphore() -> Option<&'static Arc<Semaphore>> {
    static SEMAPHORE: OnceLock<Option<Arc<Semaphore>>> = OnceLock::new();
    SEMAPHORE
        .get_or_init(|| {
            let raw = std::env::var(ENV).ok();
            parse_limit(raw.as_deref()).map(|n| Arc::new(Semaphore::new(n)))
        })
        .as_ref()
}

/// Chromium を 1 つ起動する前に枠を取る。
///
/// # Errors
/// 待っても枠が空かないとき。
pub(crate) async fn acquire() -> Result<Slot, String> {
    acquire_from(semaphore(), WAIT).await
}

async fn acquire_from(semaphore: Option<&Arc<Semaphore>>, wait: Duration) -> Result<Slot, String> {
    let Some(semaphore) = semaphore else {
        return Ok(Slot { _permit: None });
    };
    match tokio::time::timeout(wait, Arc::clone(semaphore).acquire_owned()).await {
        Ok(Ok(permit)) => Ok(Slot {
            _permit: Some(permit),
        }),
        Ok(Err(_)) | Err(_) => Err(BUSY.to_owned()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn unset_empty_zero_and_invalid_mean_unlimited() {
        assert_eq!(parse_limit(None), None);
        assert_eq!(parse_limit(Some("")), None);
        assert_eq!(parse_limit(Some("  ")), None);
        assert_eq!(parse_limit(Some("0")), None);
        assert_eq!(parse_limit(Some("abc")), None);
        assert_eq!(parse_limit(Some("-1")), None);
    }

    #[test]
    fn positive_value_is_the_limit() {
        assert_eq!(parse_limit(Some("3")), Some(3));
        assert_eq!(parse_limit(Some(" 2 ")), Some(2));
        assert_eq!(
            parse_limit(Some(&usize::MAX.to_string())),
            Some(Semaphore::MAX_PERMITS)
        );
    }

    #[tokio::test]
    async fn unlimited_always_succeeds() {
        let a = acquire_from(None, Duration::from_millis(1)).await;
        let b = acquire_from(None, Duration::from_millis(1)).await;
        assert!(a.is_ok() && b.is_ok());
    }

    #[tokio::test]
    async fn full_limit_rejects_until_a_slot_is_released() {
        let semaphore = Arc::new(Semaphore::new(1));
        let wait = Duration::from_millis(20);
        let first = acquire_from(Some(&semaphore), wait).await.unwrap();
        let err = acquire_from(Some(&semaphore), wait).await.err().unwrap();
        assert!(err.contains("上限"));
        drop(first);
        assert!(acquire_from(Some(&semaphore), wait).await.is_ok());
    }
}
