//! yuuka-supervisor（bin `yuuka`）— エントリポイント兼監督。
//!
//! Phase 0 stub。実装は Phase 3+（自前 `JoinSet` + `join_next()` 監督・指数バックオフ
//! 再 spawn・`panic="unwind"` 前提。`docs/rust-rewrite/parts/02-errors-resilience.md`）。
//! ここではまだ常駐せず、契約が凍結されたことを示すだけの stub。

fn main() {
    // Phase 0: web/discord/services を JoinSet で束ねる監督ループはまだ無い。
    eprintln!("yuuka: supervisor stub (Phase 0). foundation contracts frozen; daemon not yet implemented.");
}
