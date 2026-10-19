//! `config.yaml` 専用の最小 YAML ローダ（safe Rust の `granit-parser` イベント → 値ツリー）。
//!
//! 旧実装は `serde_yaml` 0.9（unsafe-libyaml・RUSTSEC-2024-0449）の `Mapping` へ読んでいた。
//! 移行で設定の読み方が変わらないよう、構文解析だけを `granit-parser` に任せ、値の解釈は
//! serde_yaml 0.9.34 の `Mapping`/`Value` デシリアライズ規則をそのまま移植している:
//!
//! - プレーンスカラーの解決順（空/`~`/`null`→null、`true`/`True`/`TRUE`→bool、整数、浮動小数、
//!   それ以外は文字列）。`yes`/`on` は文字列、`0123` のような先頭 0 の数字列も文字列
//!   （YAML 1.2）。u64/i64 に収まらない整数は旧実装同様パース失敗にする。
//! - 引用・ブロックスカラーは常に文字列。`!!str`/`!!binary` 等のグローバルタグは文字列、
//!   `!!int`/`!!float`/`!!bool`/`!!null` は厳密に解釈（不一致はパース失敗）、`!foo` 等の
//!   ローカルタグ付きノードは「タグ付き値」（設定値としては読まれない）。
//! - マージキー `<<` は展開せず通常のキー。重複キーはパース失敗。
//! - ルートは mapping（または空ドキュメント）のみ。複数ドキュメントはパース失敗。
//! - 入れ子上限 128・エイリアス展開回数上限（イベント数×100）。
//!
//! 数値の文字列化（[`Number`] の `Display`）も旧実装と同じ（浮動小数は `ryu`）。

use std::collections::HashMap;
use std::fmt;

use granit_parser::{Event, Marker, Parser, ScalarStyle, ScanError, StrInput, Tag};

/// serde_yaml 0.9 の `remaining_depth` 初期値（入れ子コレクション数の上限）。
const RECURSION_LIMIT: u8 = 128;
/// serde_yaml 0.9 のエイリアス展開回数上限の係数（イベント数×この値）。
const REPETITION_FACTOR: usize = 100;

const TAG_NULL: &str = "tag:yaml.org,2002:null";
const TAG_BOOL: &str = "tag:yaml.org,2002:bool";
const TAG_INT: &str = "tag:yaml.org,2002:int";
const TAG_FLOAT: &str = "tag:yaml.org,2002:float";

/// `config.yaml` のパース失敗。メッセージに設定値（機密を含み得る）は含めない。
#[derive(Debug)]
pub struct YamlError {
    message: String,
    location: Option<(usize, usize)>,
}

impl YamlError {
    fn new(message: impl Into<String>, mark: Option<Marker>) -> Self {
        Self {
            message: message.into(),
            location: mark.map(|m| (m.line(), m.col() + 1)),
        }
    }

    fn scan(err: &ScanError) -> Self {
        Self::new(err.kind().to_string(), Some(*err.marker()))
    }
}

impl fmt::Display for YamlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)?;
        if let Some((line, column)) = self.location {
            write!(f, " at line {line} column {column}")?;
        }
        Ok(())
    }
}

impl std::error::Error for YamlError {}

/// YAML の値（serde_yaml 0.9 の `Value` 相当）。
#[derive(Debug, Clone)]
pub(crate) enum Value {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Sequence(Vec<Value>),
    Mapping(Mapping),
    /// ローカルタグ（`!foo`）付きの値。タグは `!` を除いた形で持つ。
    Tagged(String, Box<Value>),
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Null, Self::Null) => true,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Number(a), Self::Number(b)) => a == b,
            (Self::String(a), Self::String(b)) => a == b,
            (Self::Sequence(a), Self::Sequence(b)) => a == b,
            (Self::Mapping(a), Self::Mapping(b)) => a == b,
            (Self::Tagged(ta, a), Self::Tagged(tb, b)) => nobang(ta) == nobang(tb) && a == b,
            _ => false,
        }
    }
}

/// 数値（serde_yaml 0.9 の `Number` 相当）。
#[derive(Debug, Clone, Copy)]
pub(crate) enum Number {
    PosInt(u64),
    NegInt(i64),
    Float(f64),
}

impl From<i64> for Number {
    fn from(i: i64) -> Self {
        u64::try_from(i).map_or(Self::NegInt(i), Self::PosInt)
    }
}

impl PartialEq for Number {
    fn eq(&self, other: &Self) -> bool {
        match (*self, *other) {
            (Self::PosInt(a), Self::PosInt(b)) => a == b,
            (Self::NegInt(a), Self::NegInt(b)) => a == b,
            // YAML の NaN は 1 種類として等しく扱う（serde_yaml と同じ）。
            (Self::Float(a), Self::Float(b)) => a == b || (a.is_nan() && b.is_nan()),
            _ => false,
        }
    }
}

impl fmt::Display for Number {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::PosInt(i) => write!(f, "{i}"),
            Self::NegInt(i) => write!(f, "{i}"),
            Self::Float(x) if x.is_nan() => f.write_str(".nan"),
            Self::Float(x) if x.is_infinite() => f.write_str(if x.is_sign_negative() {
                "-.inf"
            } else {
                ".inf"
            }),
            Self::Float(x) => f.write_str(ryu::Buffer::new().format_finite(x)),
        }
    }
}

/// 挿入順を保つ mapping（重複キーは構築時に拒否済み）。
#[derive(Debug, Clone, Default)]
pub(crate) struct Mapping(Vec<(Value, Value)>);

impl Mapping {
    /// 文字列キー `key` の値。
    pub(crate) fn get(&self, key: &str) -> Option<&Value> {
        self.0
            .iter()
            .find(|(k, _)| matches!(k, Value::String(s) if s == key))
            .map(|(_, v)| v)
    }

    fn contains_key(&self, key: &Value) -> bool {
        self.0.iter().any(|(k, _)| k == key)
    }
}

impl PartialEq for Mapping {
    /// キーの順序に依らない比較（serde_yaml の `Mapping` = IndexMap と同じ）。
    fn eq(&self, other: &Self) -> bool {
        self.0.len() == other.0.len()
            && self
                .0
                .iter()
                .all(|(k, v)| other.0.iter().any(|(ok, ov)| ok == k && ov == v))
    }
}

/// `src` をルート mapping として読む（空ドキュメントは空 mapping）。
pub(crate) fn from_str(src: &str) -> Result<Mapping, YamlError> {
    let mut parser = Parser::new_from_str(src);
    let document = load_document(&mut parser);
    let mut de = Composer {
        events: &document.events,
        jumpcount: 0,
    };
    let mapping = de.root()?;
    if let Some(err) = document.error {
        return Err(err);
    }
    if has_more_documents(&mut parser) {
        return Err(YamlError::new(
            "deserializing from YAML containing more than one document is not supported",
            None,
        ));
    }
    Ok(mapping)
}

/// 合成用に正規化したイベント。エイリアスはアンカー定義イベントの位置を指す。
enum Ev {
    Alias(usize),
    Scalar {
        value: String,
        plain: bool,
        tag: Option<String>,
    },
    SequenceStart {
        tag: Option<String>,
    },
    SequenceEnd,
    MappingStart {
        tag: Option<String>,
    },
    MappingEnd,
    /// イベント切れ（空ストリーム・パース失敗で途切れた位置）。
    Void,
}

struct Document {
    events: Vec<(Ev, Marker)>,
    error: Option<YamlError>,
}

/// 最初のドキュメントのイベントを読み切る（serde_yaml の `Loader::next_document` 相当）。
fn load_document<'a>(parser: &mut Parser<'a, StrInput<'a>>) -> Document {
    let mut doc = Document {
        events: Vec::new(),
        error: None,
    };
    // アンカー ID（0 は無し）→ 定義イベントの位置。
    let mut anchors: HashMap<usize, usize> = HashMap::new();
    loop {
        let (event, span) = match parser.next() {
            Some(Ok(next)) => next,
            Some(Err(err)) => {
                doc.error = Some(YamlError::scan(&err));
                return doc;
            }
            None => {
                if doc.events.is_empty() {
                    doc.events.push((Ev::Void, Marker::new(0, 1, 0)));
                }
                return doc;
            }
        };
        let mark = span.start;
        let pos = doc.events.len();
        let ev = match event {
            Event::StreamStart | Event::DocumentStart(..) => continue,
            Event::StreamEnd => {
                if doc.events.is_empty() {
                    doc.events.push((Ev::Void, mark));
                }
                return doc;
            }
            Event::DocumentEnd => return doc,
            Event::Alias(id) => match anchors.get(&id) {
                Some(&target) => Ev::Alias(target),
                None => {
                    doc.error = Some(YamlError::new("unknown anchor", Some(mark)));
                    return doc;
                }
            },
            Event::Scalar(value, style, anchor, tag) => {
                define_anchor(&mut anchors, anchor, pos);
                let plain = matches!(style, ScalarStyle::Plain);
                // 内容の無いノードを granit は長さ 0 の `~` で表すが、libyaml は空文字列で表す
                // （ルートの `---` だけの文書を空 mapping として受け付ける判定が依存する）。
                let synthetic_null = plain && tag.is_none() && span.is_empty() && value == "~";
                Ev::Scalar {
                    value: if synthetic_null {
                        String::new()
                    } else {
                        value.into_owned()
                    },
                    plain,
                    tag: tag.as_deref().map(full_tag),
                }
            }
            Event::SequenceStart(_, anchor, tag) => {
                define_anchor(&mut anchors, anchor, pos);
                Ev::SequenceStart {
                    tag: tag.as_deref().map(full_tag),
                }
            }
            Event::SequenceEnd => Ev::SequenceEnd,
            Event::MappingStart(_, anchor, tag) => {
                define_anchor(&mut anchors, anchor, pos);
                Ev::MappingStart {
                    tag: tag.as_deref().map(full_tag),
                }
            }
            Event::MappingEnd => Ev::MappingEnd,
            // コメント等の表示用イベント（`comments` feature が他から有効化された場合）は読み飛ばす。
            _ => continue,
        };
        doc.events.push((ev, mark));
    }
}

fn define_anchor(anchors: &mut HashMap<usize, usize>, id: usize, pos: usize) {
    if id != 0 {
        anchors.insert(id, pos);
    }
}

/// 最初のドキュメントの後に別ドキュメント（または構文エラー）が続くか。
fn has_more_documents<'a>(parser: &mut Parser<'a, StrInput<'a>>) -> bool {
    loop {
        match parser.next() {
            None | Some(Ok((Event::StreamEnd, _))) => return false,
            Some(Err(_)) => return true,
            Some(Ok((event, _))) => {
                if matches!(
                    event,
                    Event::DocumentStart(..)
                        | Event::Alias(_)
                        | Event::Scalar(..)
                        | Event::SequenceStart(..)
                        | Event::MappingStart(..)
                ) {
                    return true;
                }
            }
        }
    }
}

/// libyaml と同じ完全形のタグ（`!!str` → `tag:yaml.org,2002:str`、`!foo` → `!foo`）。
fn full_tag(tag: &Tag) -> String {
    format!("{}{}", tag.handle(), tag.suffix())
}

/// `!` 始まりのタグ（ローカルタグ）なら `!` を除いた名前（serde_yaml の `parse_tag`）。
fn local_tag(tag: Option<&str>) -> Option<&str> {
    let rest = tag?.strip_prefix('!')?;
    Some(if rest.is_empty() { "!" } else { rest })
}

fn nobang(tag: &str) -> &str {
    match tag.strip_prefix('!') {
        Some("") | None => tag,
        Some(unbanged) => unbanged,
    }
}

static VOID: Ev = Ev::Void;

struct Composer<'a> {
    events: &'a [(Ev, Marker)],
    jumpcount: usize,
}

impl<'a> Composer<'a> {
    fn event(&self, pos: usize) -> (&'a Ev, Option<Marker>) {
        match self.events.get(pos) {
            Some((ev, mark)) => (ev, Some(*mark)),
            None => (&VOID, None),
        }
    }

    /// ルート（serde_yaml の `deserialize_map` + `Mapping` visitor）。
    fn root(&mut self) -> Result<Mapping, YamlError> {
        let mut pos = 0;
        let (ev, mark) = self.event(pos);
        pos += 1;
        match ev {
            Ev::MappingStart { .. } => self.mapping(&mut pos, RECURSION_LIMIT, mark),
            Ev::Void => Ok(Mapping::default()),
            Ev::Scalar { value, plain, .. } if value.is_empty() && *plain => Ok(Mapping::default()),
            _ => Err(YamlError::new(
                "invalid type, expected a YAML mapping",
                mark,
            )),
        }
    }

    /// 1 ノードを値にする（serde_yaml の `deserialize_any` + `Value` visitor）。
    fn node(
        &mut self,
        pos: &mut usize,
        depth: u8,
        tagged_already: bool,
    ) -> Result<Value, YamlError> {
        let start = *pos;
        let (ev, mark) = self.event(start);
        *pos += 1;
        match ev {
            Ev::Alias(target) => {
                self.jumpcount += 1;
                if self.jumpcount > self.events.len().saturating_mul(REPETITION_FACTOR) {
                    return Err(YamlError::new("repetition limit exceeded", mark));
                }
                let mut alias_pos = *target;
                self.node(&mut alias_pos, depth, false)
            }
            Ev::Scalar { tag, .. } | Ev::SequenceStart { tag } | Ev::MappingStart { tag }
                if !tagged_already && local_tag(tag.as_deref()).is_some() =>
            {
                let name = local_tag(tag.as_deref()).unwrap_or_default().to_owned();
                *pos = start;
                let inner = self.node(pos, depth, true)?;
                Ok(Value::Tagged(name, Box::new(inner)))
            }
            Ev::Scalar { value, plain, tag } => {
                let tag = if tagged_already { None } else { tag.as_deref() };
                resolve_scalar(value, *plain, tag).map_err(|msg| YamlError::new(msg, mark))
            }
            Ev::SequenceStart { .. } => {
                let depth = next_depth(depth, mark)?;
                let mut seq = Vec::new();
                while !self.at_end(*pos, true) {
                    seq.push(self.node(pos, depth, false)?);
                }
                *pos += 1;
                Ok(Value::Sequence(seq))
            }
            Ev::MappingStart { .. } => self.mapping(pos, depth, mark).map(Value::Mapping),
            Ev::Void => Ok(Value::Null),
            Ev::SequenceEnd | Ev::MappingEnd => {
                Err(YamlError::new("unexpected end of collection", mark))
            }
        }
    }

    /// MappingStart の直後から MappingEnd までを読む（重複キーはエラー）。
    fn mapping(
        &mut self,
        pos: &mut usize,
        depth: u8,
        mark: Option<Marker>,
    ) -> Result<Mapping, YamlError> {
        let depth = next_depth(depth, mark)?;
        let mut mapping = Mapping::default();
        while !self.at_end(*pos, false) {
            let (_, key_mark) = self.event(*pos);
            let key = self.node(pos, depth, false)?;
            if mapping.contains_key(&key) {
                return Err(YamlError::new("duplicate entry in YAML map", key_mark));
            }
            let value = self.node(pos, depth, false)?;
            mapping.0.push((key, value));
        }
        *pos += 1;
        Ok(mapping)
    }

    /// コレクション終端（または途切れ）か。
    fn at_end(&self, pos: usize, sequence: bool) -> bool {
        match self.event(pos).0 {
            Ev::Void => true,
            Ev::SequenceEnd => sequence,
            Ev::MappingEnd => !sequence,
            _ => false,
        }
    }
}

fn next_depth(depth: u8, mark: Option<Marker>) -> Result<u8, YamlError> {
    depth
        .checked_sub(1)
        .ok_or_else(|| YamlError::new("recursion limit exceeded", mark))
}

/// スカラーの解決（serde_yaml 0.9 の `visit_scalar`）。`tag` はタグ処理済みなら `None`。
fn resolve_scalar(v: &str, plain: bool, tag: Option<&str>) -> Result<Value, &'static str> {
    match tag {
        Some(TAG_BOOL) => parse_bool(v)
            .map(Value::Bool)
            .ok_or("invalid value, expected a boolean"),
        Some(TAG_INT) => match parse_int(v) {
            Some(Ok(n)) => Ok(Value::Number(n)),
            _ => Err("invalid value, expected an integer"),
        },
        Some(TAG_FLOAT) => parse_f64(v)
            .map(|f| Value::Number(Number::Float(f)))
            .ok_or("invalid value, expected a float"),
        Some(TAG_NULL) => parse_null(v)
            .map(|()| Value::Null)
            .ok_or("invalid value, expected null"),
        Some(t) if t.starts_with('!') && plain => resolve_plain(v),
        None if plain => resolve_plain(v),
        _ => Ok(Value::String(v.to_owned())),
    }
}

/// タグ無しプレーンスカラーの解決（serde_yaml 0.9 の `visit_untagged_scalar`）。
fn resolve_plain(v: &str) -> Result<Value, &'static str> {
    if v.is_empty() || parse_null(v).is_some() {
        return Ok(Value::Null);
    }
    if let Some(b) = parse_bool(v) {
        return Ok(Value::Bool(b));
    }
    if let Some(n) = parse_int(v) {
        return n.map(Value::Number);
    }
    if !digits_but_not_number(v) {
        if let Some(f) = parse_f64(v) {
            return Ok(Value::Number(Number::Float(f)));
        }
    }
    Ok(Value::String(v.to_owned()))
}

fn parse_null(v: &str) -> Option<()> {
    matches!(v, "null" | "Null" | "NULL" | "~").then_some(())
}

fn parse_bool(v: &str) -> Option<bool> {
    match v {
        "true" | "True" | "TRUE" => Some(true),
        "false" | "False" | "FALSE" => Some(false),
        _ => None,
    }
}

/// 整数（serde_yaml 0.9 の `visit_int`）。u64/i64 に収まらず u128/i128 に収まる整数は、
/// 旧実装の `Value` が受け付けずパース失敗になっていたため `Some(Err(_))` を返す。
fn parse_int(v: &str) -> Option<Result<Number, &'static str>> {
    if let Some(i) = parse_unsigned_int(v, u64::from_str_radix) {
        return Some(Ok(Number::PosInt(i)));
    }
    if let Some(i) = parse_negative_int(v, i64::from_str_radix) {
        return Some(Ok(Number::from(i)));
    }
    if parse_unsigned_int(v, u128::from_str_radix).is_some()
        || parse_negative_int(v, i128::from_str_radix).is_some()
    {
        return Some(Err("invalid type: integer out of 64-bit range"));
    }
    None
}

type FromStrRadix<T> = fn(&str, u32) -> Result<T, std::num::ParseIntError>;

fn parse_unsigned_int<T>(v: &str, from_str_radix: FromStrRadix<T>) -> Option<T> {
    let unpositive = v.strip_prefix('+').unwrap_or(v);
    for (prefix, radix) in [("0x", 16), ("0o", 8), ("0b", 2)] {
        if let Some(rest) = unpositive.strip_prefix(prefix) {
            if rest.starts_with(['+', '-']) {
                return None;
            }
            if let Ok(i) = from_str_radix(rest, radix) {
                return Some(i);
            }
        }
    }
    if unpositive.starts_with(['+', '-']) || digits_but_not_number(v) {
        return None;
    }
    from_str_radix(unpositive, 10).ok()
}

fn parse_negative_int<T>(v: &str, from_str_radix: FromStrRadix<T>) -> Option<T> {
    for (prefix, radix) in [("-0x", 16), ("-0o", 8), ("-0b", 2)] {
        if let Some(rest) = v.strip_prefix(prefix) {
            if let Ok(i) = from_str_radix(&format!("-{rest}"), radix) {
                return Some(i);
            }
        }
    }
    if digits_but_not_number(v) {
        return None;
    }
    from_str_radix(v, 10).ok()
}

fn parse_f64(v: &str) -> Option<f64> {
    let unpositive = match v.strip_prefix('+') {
        Some(rest) if rest.starts_with(['+', '-']) => return None,
        Some(rest) => rest,
        None => v,
    };
    if matches!(unpositive, ".inf" | ".Inf" | ".INF") {
        return Some(f64::INFINITY);
    }
    if matches!(v, "-.inf" | "-.Inf" | "-.INF") {
        return Some(f64::NEG_INFINITY);
    }
    if matches!(v, ".nan" | ".NaN" | ".NAN") {
        return Some(f64::NAN);
    }
    unpositive.parse::<f64>().ok().filter(|f| f.is_finite())
}

/// 先頭 0 に数字が続く列は YAML 1.2 では数値ではなく文字列。
fn digits_but_not_number(v: &str) -> bool {
    let v = v.strip_prefix(['-', '+']).unwrap_or(v);
    v.len() > 1 && v.starts_with('0') && v.bytes().skip(1).all(|b| b.is_ascii_digit())
}
