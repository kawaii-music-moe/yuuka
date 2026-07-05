//! yuuka-supervisor（bin `yuuka`）— エントリポイント兼監督。
//!
//! Phase 0 stub。実装は Phase 3+（自前 `JoinSet` + `join_next()` 監督・指数バックオフ
//! 再 spawn・`panic="unwind"` 前提。`docs/rust-rewrite/parts/02-errors-resilience.md`）。
//! ここではまだ常駐せず、契約が凍結されたことを示すだけの stub。

fn main() {
    // Phase 1: アプリ組立は `yuuka_supervisor::build_app`（web + todo + 共通レイヤ）。
    // config 読込・AuthBackend 実装・axum::serve・JoinSet 監督は後続増分で配線する。
    eprintln!(
        "yuuka: supervisor (Phase 1). app assembly available (build_app); config/serve/supervision not yet wired."
    );
}
