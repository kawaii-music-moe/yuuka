//! チャート描画用フォントのロード（ab_glyph・純 Rust・native freetype/fontconfig 非依存）。
//!
//! 日本語ラベルのため fonts-noto-cjk（Docker 同梱・`.ttc`）を優先し、無ければ DejaVu（Latin のみ・
//! テスト/フォールバック）を探す。`YUUKA_CHART_FONT` で明示パス指定可。見つからなければ `None`
//! （描画はテキスト無しで続行＝クラッシュしない）。プロセス内で 1 回だけロードしてキャッシュする。

use std::sync::OnceLock;

use ab_glyph::FontVec;

/// フォント探索の既定パス（noto-cjk `.ttc` → DejaVu `.ttf` フォールバック）。
const CANDIDATES: &[&str] = &[
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Medium.ttc",
    "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
];

static FONT: OnceLock<Option<FontVec>> = OnceLock::new();

/// 共有フォントを返す（初回のみロード）。見つからなければ `None`。
pub(crate) fn shared() -> Option<&'static FontVec> {
    FONT.get_or_init(load).as_ref()
}

fn load() -> Option<FontVec> {
    let mut paths: Vec<String> = Vec::new();
    if let Ok(p) = std::env::var("YUUKA_CHART_FONT") {
        if !p.is_empty() {
            paths.push(p);
        }
    }
    paths.extend(CANDIDATES.iter().map(|s| (*s).to_owned()));

    for path in paths {
        if let Ok(bytes) = std::fs::read(&path) {
            // `.ttc`（TrueType Collection）も index 0 で先頭フォントを取り出せる。
            if let Ok(font) = FontVec::try_from_vec_and_index(bytes, 0) {
                return Some(font);
            }
        }
    }
    None
}
