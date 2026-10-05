//! yuuka-chart — sendChart ツール（Node `chartFunctions`/`chartService` パリティ）。
//!
//! 数値データを暗色テーマの PNG グラフ（pie/doughnut/bar/horizontalBar/line）にして返信へ添付する。
//! 描画は純 Rust（`image` + `ab_glyph`・native freetype/fontconfig 非依存）。日本語ラベルは Docker
//! 同梱の fonts-noto-cjk（`.ttc`）を直接ロードする（[`font`]）。
//!
//! 依存の向き: supervisor → yuuka-chart → yuuka-core。ツール露出は既定（secretary）。

mod font;
mod render;
mod tools;

pub use tools::tools;
