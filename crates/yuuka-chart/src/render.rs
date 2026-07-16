//! チャート描画（image + ab_glyph・純 Rust）。暗色テーマで PNG bytes を返す。
//!
//! 対応: pie / doughnut / bar / horizontalBar / line（Node `chartService.renderChart` の 5 種）。
//! Node は chart.js のため見た目の厳密一致はしないが、暗色背景・系列色・タイトル・ラベル・値・
//! 2 系列比較（`second_values`）を再現する。テキストはフォント未検出時のみ省略（クラッシュ回避）。

use ab_glyph::{Font, PxScale, ScaleFont};
use image::{ExtendedColorType, ImageEncoder, Rgba, RgbaImage};

use crate::font;

const W: u32 = 900;
const H: u32 = 520;
/// 暗色背景（§3.0.2 データ系パープル運用に合わせた dark）。
const BG: [u8; 4] = [0x2b, 0x2d, 0x31, 0xff];
const FG: [u8; 4] = [0xdc, 0xdd, 0xde, 0xff];
const MUTED: [u8; 4] = [0x9a, 0x9c, 0xa0, 0xff];
const GRID: [u8; 4] = [0x3f, 0x42, 0x47, 0xff];
/// 系列色パレット（暗色背景で映える）。
const PALETTE: &[[u8; 4]] = &[
    [0x5b, 0x8f, 0xf9, 0xff], // blue
    [0x9b, 0x59, 0xb6, 0xff], // purple
    [0x2e, 0xcc, 0x71, 0xff], // green
    [0xe6, 0x7e, 0x22, 0xff], // orange
    [0xe7, 0x4c, 0x3c, 0xff], // red
    [0x1a, 0xbc, 0x9c, 0xff], // teal
    [0xf1, 0xc4, 0x0f, 0xff], // yellow
    [0xe8, 0x4b, 0x8a, 0xff], // pink
];

/// 描画するチャートの種別。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ChartType {
    Pie,
    Doughnut,
    Bar,
    HorizontalBar,
    Line,
}

impl ChartType {
    /// Node の type 文字列から解決する（未知は `None`）。
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pie" => Some(Self::Pie),
            "doughnut" => Some(Self::Doughnut),
            "bar" => Some(Self::Bar),
            "horizontalBar" => Some(Self::HorizontalBar),
            "line" => Some(Self::Line),
            _ => None,
        }
    }
}

/// 1 系列（ラベル `series_label` と値）。
pub struct Series {
    pub label: Option<String>,
    pub values: Vec<f64>,
}

/// チャート描画リクエスト。
pub struct ChartSpec {
    pub kind: ChartType,
    pub title: String,
    pub labels: Vec<String>,
    pub primary: Series,
    pub secondary: Option<Series>,
}

/// PNG bytes を生成する。
///
/// # Errors
/// PNG エンコード失敗時。
pub fn render(spec: &ChartSpec) -> Result<Vec<u8>, String> {
    let mut img = RgbaImage::from_pixel(W, H, Rgba(BG));
    let f = font::shared();

    // タイトル（上部中央）。
    draw_text_centered(&mut img, f, W as f32 / 2.0, 12.0, 30.0, &spec.title, FG);

    match spec.kind {
        ChartType::Pie | ChartType::Doughnut => draw_pie(&mut img, f, spec),
        ChartType::HorizontalBar => draw_hbar(&mut img, f, spec),
        ChartType::Line => draw_line_chart(&mut img, f, spec),
        ChartType::Bar => draw_bar(&mut img, f, spec),
    }

    let mut buf = Vec::new();
    image::codecs::png::PngEncoder::new(&mut buf)
        .write_image(img.as_raw(), W, H, ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())?;
    Ok(buf)
}

// ─── 描画プリミティブ ─────────────────────────────────────────────────────────

/// 範囲チェック付きのアルファ合成（coverage 0.0..=1.0）。
fn blend(img: &mut RgbaImage, x: i32, y: i32, color: [u8; 4], coverage: f32) {
    if x < 0 || y < 0 || x >= W as i32 || y >= H as i32 || coverage <= 0.0 {
        return;
    }
    let (xu, yu) = (x as u32, y as u32);
    let a = coverage.clamp(0.0, 1.0) * (f32::from(color[3]) / 255.0);
    let existing = img.get_pixel(xu, yu).0;
    let mix = |fg: u8, bg: u8| -> u8 {
        (f32::from(fg) * a + f32::from(bg) * (1.0 - a))
            .round()
            .clamp(0.0, 255.0) as u8
    };
    img.put_pixel(
        xu,
        yu,
        Rgba([
            mix(color[0], existing[0]),
            mix(color[1], existing[1]),
            mix(color[2], existing[2]),
            0xff,
        ]),
    );
}

/// 矩形塗り（x0..x1, y0..y1）。
fn fill_rect(img: &mut RgbaImage, x0: i32, y0: i32, x1: i32, y1: i32, color: [u8; 4]) {
    let (lo_x, hi_x) = (x0.min(x1), x0.max(x1));
    let (lo_y, hi_y) = (y0.min(y1), y0.max(y1));
    for y in lo_y..hi_y {
        for x in lo_x..hi_x {
            blend(img, x, y, color, 1.0);
        }
    }
}

/// 直線（Bresenham）。
fn draw_line(img: &mut RgbaImage, x0: i32, y0: i32, x1: i32, y1: i32, color: [u8; 4]) {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let (mut x, mut y) = (x0, y0);
    loop {
        blend(img, x, y, color, 1.0);
        // 少し太らせる（見やすさ）。
        blend(img, x + 1, y, color, 0.5);
        blend(img, x, y + 1, color, 0.5);
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

/// 円（塗り）セクター: 中心 (cx,cy)・半径 r・角度 [a0,a1)（ラジアン・時計回り）。
/// `inner` > 0 でドーナツ（内側をくり抜く）。
fn fill_sector(
    img: &mut RgbaImage,
    cx: f32,
    cy: f32,
    r: f32,
    inner: f32,
    (a0, a1): (f32, f32),
    color: [u8; 4],
) {
    let r_i = r.ceil() as i32;
    for dy in -r_i..=r_i {
        for dx in -r_i..=r_i {
            let (fx, fy) = (dx as f32, dy as f32);
            let dist = (fx * fx + fy * fy).sqrt();
            if dist > r || dist < inner {
                continue;
            }
            // atan2 を [0, 2π) の時計回り角へ。
            let mut ang = fy.atan2(fx);
            if ang < 0.0 {
                ang += std::f32::consts::TAU;
            }
            if ang >= a0 && ang < a1 {
                // 縁のアンチエイリアス（外周 1px）。
                let cov = if r - dist < 1.0 { r - dist } else { 1.0 };
                blend(img, (cx + fx) as i32, (cy + fy) as i32, color, cov.max(0.0));
            }
        }
    }
}

/// テキスト描画（ab_glyph）。フォント未検出（`None`）なら何もしない。
fn draw_text(
    img: &mut RgbaImage,
    f: Option<&impl Font>,
    x: f32,
    y: f32,
    size: f32,
    text: &str,
    color: [u8; 4],
) {
    let Some(font) = f else { return };
    let scale = PxScale::from(size);
    let scaled = font.as_scaled(scale);
    let ascent = scaled.ascent();
    let mut caret = x;
    for ch in text.chars() {
        let gid = font.glyph_id(ch);
        let glyph = gid.with_scale_and_position(scale, ab_glyph::point(caret, y + ascent));
        if let Some(outline) = font.outline_glyph(glyph) {
            let bounds = outline.px_bounds();
            outline.draw(|gx, gy, cov| {
                blend(
                    img,
                    bounds.min.x as i32 + gx as i32,
                    bounds.min.y as i32 + gy as i32,
                    color,
                    cov,
                );
            });
        }
        caret += scaled.h_advance(gid);
    }
}

/// テキスト幅（px）。
fn text_width(font: &impl Font, size: f32, text: &str) -> f32 {
    let scaled = font.as_scaled(PxScale::from(size));
    text.chars()
        .map(|c| scaled.h_advance(font.glyph_id(c)))
        .sum()
}

/// 中央寄せテキスト（x 中心）。
fn draw_text_centered(
    img: &mut RgbaImage,
    f: Option<&impl Font>,
    cx: f32,
    y: f32,
    size: f32,
    s: &str,
    color: [u8; 4],
) {
    let w = f.map_or(0.0, |font| text_width(font, size, s));
    draw_text(img, f, cx - w / 2.0, y, size, s, color);
}

/// パレット色を巡回で取る。
fn palette(i: usize) -> [u8; 4] {
    *PALETTE.get(i % PALETTE.len()).unwrap_or(&FG)
}

/// 値の短い表示（整数なら小数点無し）。
fn fmt_value(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.1}")
    }
}

// ─── 各チャート ───────────────────────────────────────────────────────────────

/// 縦棒（bar・`second_values` があれば 2 系列を横並び）。
fn draw_bar(img: &mut RgbaImage, f: Option<&impl Font>, spec: &ChartSpec) {
    let (left, right, top, bottom) = (70.0_f32, W as f32 - 30.0, 70.0_f32, H as f32 - 60.0);
    let series: Vec<&Series> = std::iter::once(&spec.primary)
        .chain(spec.secondary.iter())
        .collect();
    let max = series
        .iter()
        .flat_map(|s| s.values.iter())
        .fold(0.0_f64, |m, v| m.max(*v))
        .max(1.0);

    axis(img, left, right, top, bottom);
    // 目盛（4 分割）。
    for k in 0..=4 {
        let y = bottom - (bottom - top) * (k as f32 / 4.0);
        draw_line(img, left as i32, y as i32, right as i32, y as i32, GRID);
        draw_text_centered(
            img,
            f,
            left - 22.0,
            y - 8.0,
            16.0,
            &fmt_value(max * f64::from(k) / 4.0),
            MUTED,
        );
    }

    let n = spec.labels.len().max(1);
    let slot = (right - left) / n as f32;
    let sub = series.len().max(1);
    let bw = (slot * 0.7) / sub as f32;
    for (i, label) in spec.labels.iter().enumerate() {
        let base_x = left + slot * i as f32 + slot * 0.15;
        for (si, s) in series.iter().enumerate() {
            let v = s.values.get(i).copied().unwrap_or(0.0);
            let h = ((v / max) as f32) * (bottom - top);
            let x0 = base_x + bw * si as f32;
            fill_rect(
                img,
                x0 as i32,
                (bottom - h) as i32,
                (x0 + bw) as i32,
                bottom as i32,
                palette(si),
            );
            draw_text_centered(
                img,
                f,
                x0 + bw / 2.0,
                bottom - h - 18.0,
                14.0,
                &fmt_value(v),
                FG,
            );
        }
        draw_text_centered(
            img,
            f,
            base_x + (slot * 0.7) / 2.0,
            bottom + 6.0,
            15.0,
            label,
            MUTED,
        );
    }
    legend(img, f, &series, right);
}

/// 横棒（horizontalBar・予算消化率などのプログレスバー風）。
fn draw_hbar(img: &mut RgbaImage, f: Option<&impl Font>, spec: &ChartSpec) {
    let (left, right, top, bottom) = (140.0_f32, W as f32 - 90.0, 80.0_f32, H as f32 - 40.0);
    let max = spec
        .primary
        .values
        .iter()
        .fold(0.0_f64, |m, v| m.max(*v))
        .max(1.0);
    let n = spec.labels.len().max(1);
    let slot = (bottom - top) / n as f32;
    let bh = slot * 0.6;
    for (i, label) in spec.labels.iter().enumerate() {
        let y = top + slot * i as f32 + (slot - bh) / 2.0;
        let v = spec.primary.values.get(i).copied().unwrap_or(0.0);
        let w = ((v / max) as f32) * (right - left);
        // トラック（背景）。
        fill_rect(
            img,
            left as i32,
            y as i32,
            right as i32,
            (y + bh) as i32,
            GRID,
        );
        fill_rect(
            img,
            left as i32,
            y as i32,
            (left + w) as i32,
            (y + bh) as i32,
            palette(i),
        );
        draw_text(img, f, 10.0, y + bh / 2.0 - 8.0, 15.0, label, MUTED);
        draw_text(
            img,
            f,
            right + 6.0,
            y + bh / 2.0 - 8.0,
            15.0,
            &fmt_value(v),
            FG,
        );
    }
}

/// 折れ線（line・時系列・`second_values` で 2 系列）。
fn draw_line_chart(img: &mut RgbaImage, f: Option<&impl Font>, spec: &ChartSpec) {
    let (left, right, top, bottom) = (70.0_f32, W as f32 - 30.0, 70.0_f32, H as f32 - 60.0);
    let series: Vec<&Series> = std::iter::once(&spec.primary)
        .chain(spec.secondary.iter())
        .collect();
    let max = series
        .iter()
        .flat_map(|s| s.values.iter())
        .fold(0.0_f64, |m, v| m.max(*v))
        .max(1.0);

    axis(img, left, right, top, bottom);
    for k in 0..=4 {
        let y = bottom - (bottom - top) * (k as f32 / 4.0);
        draw_line(img, left as i32, y as i32, right as i32, y as i32, GRID);
        draw_text_centered(
            img,
            f,
            left - 22.0,
            y - 8.0,
            16.0,
            &fmt_value(max * f64::from(k) / 4.0),
            MUTED,
        );
    }

    let n = spec.labels.len().max(2);
    let step = (right - left) / (n - 1).max(1) as f32;
    for (si, s) in series.iter().enumerate() {
        let color = palette(si);
        let mut prev: Option<(i32, i32)> = None;
        for (i, v) in s.values.iter().enumerate() {
            let x = left + step * i as f32;
            let y = bottom - ((*v / max) as f32) * (bottom - top);
            if let Some((px, py)) = prev {
                draw_line(img, px, py, x as i32, y as i32, color);
            }
            fill_rect(
                img,
                x as i32 - 3,
                y as i32 - 3,
                x as i32 + 3,
                y as i32 + 3,
                color,
            );
            prev = Some((x as i32, y as i32));
        }
    }
    for (i, label) in spec.labels.iter().enumerate() {
        let x = left + step * i as f32;
        draw_text_centered(img, f, x, bottom + 6.0, 15.0, label, MUTED);
    }
    legend(img, f, &series, right);
}

/// 円 / ドーナツ（pie/doughnut・構成比）。
fn draw_pie(img: &mut RgbaImage, f: Option<&impl Font>, spec: &ChartSpec) {
    let cx = 280.0_f32;
    let cy = (H as f32) / 2.0 + 20.0;
    let r = 170.0_f32;
    let inner = if spec.kind == ChartType::Doughnut {
        r * 0.55
    } else {
        0.0
    };
    let total: f64 = spec.primary.values.iter().filter(|v| **v > 0.0).sum();
    if total <= 0.0 {
        draw_text_centered(img, f, cx, cy - 10.0, 18.0, "データがありません", MUTED);
        return;
    }
    let mut ang = -std::f32::consts::FRAC_PI_2; // 12 時方向から。
    for (i, v) in spec.primary.values.iter().enumerate() {
        if *v <= 0.0 {
            continue;
        }
        let sweep = (*v / total) as f32 * std::f32::consts::TAU;
        let a0 = normalize_angle(ang);
        let a1 = normalize_angle(ang + sweep);
        // ラップする区間は 2 分割で描く。
        if a1 >= a0 {
            fill_sector(img, cx, cy, r, inner, (a0, a1), palette(i));
        } else {
            fill_sector(
                img,
                cx,
                cy,
                r,
                inner,
                (a0, std::f32::consts::TAU),
                palette(i),
            );
            fill_sector(img, cx, cy, r, inner, (0.0, a1), palette(i));
        }
        ang += sweep;
    }
    // 凡例（右側・ラベル + 構成比%）。
    for (i, label) in spec.labels.iter().enumerate() {
        let v = spec.primary.values.get(i).copied().unwrap_or(0.0);
        let pct = (v / total * 100.0).round() as i64;
        let y = 90.0 + i as f32 * 34.0;
        fill_rect(img, 560, y as i32, 584, y as i32 + 24, palette(i));
        draw_text(img, f, 594.0, y, 18.0, &format!("{label}  {pct}%"), FG);
    }
}

// ─── 補助 ─────────────────────────────────────────────────────────────────────

/// 角度を [0, 2π) へ。
fn normalize_angle(a: f32) -> f32 {
    let t = a % std::f32::consts::TAU;
    if t < 0.0 {
        t + std::f32::consts::TAU
    } else {
        t
    }
}

/// 軸（左・下）を描く。
fn axis(img: &mut RgbaImage, left: f32, right: f32, top: f32, bottom: f32) {
    draw_line(
        img,
        left as i32,
        top as i32,
        left as i32,
        bottom as i32,
        MUTED,
    );
    draw_line(
        img,
        left as i32,
        bottom as i32,
        right as i32,
        bottom as i32,
        MUTED,
    );
}

/// 凡例（複数系列時のみ・右上）。
fn legend(img: &mut RgbaImage, f: Option<&impl Font>, series: &[&Series], right: f32) {
    if series.len() < 2 {
        return;
    }
    for (i, s) in series.iter().enumerate() {
        let name = s.label.clone().unwrap_or_else(|| format!("系列{}", i + 1));
        let y = 74.0 + i as f32 * 26.0;
        fill_rect(
            img,
            (right - 130.0) as i32,
            y as i32,
            (right - 108.0) as i32,
            y as i32 + 18,
            palette(i),
        );
        draw_text(img, f, right - 100.0, y - 2.0, 16.0, &name, FG);
    }
}
