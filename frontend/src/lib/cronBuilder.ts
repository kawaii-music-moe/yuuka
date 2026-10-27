// cron 式ビルダー（CronBuilder.svelte）の純粋ロジック。
// バックエンド（yuuka_core::cron_util・croner）と同じ 5 フィールド（分 時 日 月 曜日）で、
// サーバのローカル時刻で評価される。croner の既定どおり、日と曜日を両方指定するとどちらか一方が
// 一致した日に実行される（OR）。

export type FieldIndex = 0 | 1 | 2 | 3 | 4;

/**
 * フィールドごとの指定方法。
 * - every: 毎回（`*`）
 * - custom / interval / range: 値・間隔・範囲を足していく。入力欄の出し分けだけで、
 *   どのモードでも追加済みの値・間隔・範囲はすべて式に入る（参考サイトと同じ）。
 */
export type FieldMode = "every" | "custom" | "interval" | "range";

export interface CronRange {
	start: number;
	end: number;
	/** 1 は間隔なし（`a-b`）、2 以上は `a-b/step` */
	step: number;
}

export interface CronFieldState {
	mode: FieldMode;
	values: number[];
	intervals: number[];
	ranges: CronRange[];
}

export type CronState = [
	CronFieldState,
	CronFieldState,
	CronFieldState,
	CronFieldState,
	CronFieldState,
];

export interface FieldSpec {
	/** タブ・内訳に出す名前 */
	name: string;
	min: number;
	max: number;
	/** 「毎◯」の表記 */
	everyLabel: string;
}

export const FIELDS: readonly FieldSpec[] = [
	{ name: "分", min: 0, max: 59, everyLabel: "毎分" },
	{ name: "時", min: 0, max: 23, everyLabel: "毎時" },
	{ name: "日", min: 1, max: 31, everyLabel: "毎日" },
	{ name: "月", min: 1, max: 12, everyLabel: "毎月" },
	{ name: "曜日", min: 0, max: 6, everyLabel: "毎曜日" },
];

export const WEEKDAYS = ["日", "月", "火", "水", "木", "金", "土"];

function emptyField(mode: FieldMode): CronFieldState {
	return { mode, values: [], intervals: [], ranges: [] };
}

/** 初期状態（`0 * * * *`）。 */
export function defaultState(): CronState {
	return [
		{ ...emptyField("custom"), values: [0] },
		emptyField("every"),
		emptyField("every"),
		emptyField("every"),
		emptyField("every"),
	];
}

const byNumber = (a: number, b: number) => a - b;

function rangeExpr(r: CronRange): string {
	return `${r.start}-${r.end}${r.step > 1 ? `/${r.step}` : ""}`;
}

/** 1 フィールド分の式。何も追加していなければ `*`。 */
export function fieldExpr(f: CronFieldState): string {
	if (f.mode === "every") return "*";
	const parts = [
		...[...f.intervals].sort(byNumber).map((v) => `*/${v}`),
		...f.ranges.map(rangeExpr),
		...[...f.values].sort(byNumber).map(String),
	];
	return parts.length > 0 ? parts.join(",") : "*";
}

export function buildCron(state: CronState): string {
	return state.map(fieldExpr).join(" ");
}

function toInt(s: string): number | null {
	return /^\d{1,2}$/.test(s) ? Number(s) : null;
}

/** 1 フィールドを分解する。ビルダーで表せない書き方（名前・L・# など）なら null。 */
function parseField(raw: string, index: FieldIndex): CronFieldState | null {
	const { min, max } = FIELDS[index];
	// 曜日の 7 は日曜（croner も同じ扱い）。単独の値だけ 0 に寄せる。
	const inBounds = (v: number) => v >= min && v <= max;
	if (raw === "*") return emptyField("every");
	const f = emptyField("custom");
	for (const part of raw.split(",")) {
		let m = part.match(/^\*\/(\d{1,2})$/);
		if (m) {
			const step = Number(m[1]);
			if (step < 1 || step > max) return null;
			if (!f.intervals.includes(step)) f.intervals.push(step);
			continue;
		}
		m = part.match(/^(\d{1,2})-(\d{1,2})(?:\/(\d{1,2}))?$/);
		if (m) {
			const [start, end] = [Number(m[1]), Number(m[2])];
			const step = m[3] === undefined ? 1 : Number(m[3]);
			if (!inBounds(start) || !inBounds(end) || start > end || step < 1)
				return null;
			f.ranges.push({ start, end, step });
			continue;
		}
		let v = toInt(part);
		if (v === null) return null;
		if (index === 4 && v === 7) v = 0;
		if (!inBounds(v)) return null;
		if (!f.values.includes(v)) f.values.push(v);
	}
	f.values.sort(byNumber);
	// 開いたときに見せる入力欄は、入っているものに合わせる
	if (f.values.length === 0 && f.ranges.length === 0) f.mode = "interval";
	else if (f.values.length === 0 && f.intervals.length === 0) f.mode = "range";
	return f;
}

/** cron 式をビルダーの状態に分解する。5 フィールドでない・表せない書き方なら null。 */
export function parseCron(expr: string): CronState | null {
	const fields = expr.trim().split(/\s+/);
	if (fields.length !== 5) return null;
	const out: CronFieldState[] = [];
	for (let i = 0; i < 5; i++) {
		const f = parseField(fields[i], i as FieldIndex);
		if (!f) return null;
		out.push(f);
	}
	return out as CronState;
}

/** 入力欄の文字列を min〜max の整数として読む（全角数字も可）。範囲外・数字以外は null。 */
export function parseBounded(
	raw: string,
	min: number,
	max: number,
): number | null {
	const s = raw
		.trim()
		.replace(/[０-９]/g, (c) => String(c.charCodeAt(0) - 0xff10));
	if (!/^\d{1,2}$/.test(s)) return null;
	const v = Number(s);
	return v >= min && v <= max ? v : null;
}

/** 選択肢・チップに出す値の表記（例: 15分 / 9時 / 1日 / 3月 / 月曜日）。 */
export function valueLabel(index: FieldIndex, v: number): string {
	return index === 4 ? `${WEEKDAYS[v]}曜日` : `${v}${FIELDS[index].name}`;
}

/** 間隔の単位（曜日は「日」で数える）。 */
const STEP_UNITS = ["分", "時間", "日", "か月", "日"];

export function intervalLabel(index: FieldIndex, step: number): string {
	return `${step}${STEP_UNITS[index]}ごと`;
}

export function rangeLabel(index: FieldIndex, r: CronRange): string {
	const body =
		index === 4
			? `${WEEKDAYS[r.start]}曜〜${WEEKDAYS[r.end]}曜`
			: `${r.start}〜${r.end}${FIELDS[index].name}`;
	return r.step > 1 ? `${body}（${r.step}${STEP_UNITS[index]}おき）` : body;
}

/** 追加済みの値・間隔・範囲を読める形で並べる（未指定なら空配列）。 */
function partLabels(index: FieldIndex, f: CronFieldState): string[] {
	return [
		...[...f.intervals].sort(byNumber).map((v) => intervalLabel(index, v)),
		...f.ranges.map((r) => rangeLabel(index, r)),
		...[...f.values].sort(byNumber).map((v) => valueLabel(index, v)),
	];
}

/** そのフィールドが実質「毎回」か（`*` と同じ）。 */
function isEvery(f: CronFieldState): boolean {
	return fieldExpr(f) === "*";
}

/** 時・分が「決まった値だけ」なら、その値の一覧。 */
function fixedValues(index: 0 | 1, f: CronFieldState): number[] | null {
	if (isEvery(f) || f.intervals.length > 0 || f.ranges.length > 0) return null;
	return [...f.values].sort(byNumber);
}

const pad = (n: number) => String(n).padStart(2, "0");

function timeText(minute: CronFieldState, hour: CronFieldState): string {
	const ms = fixedValues(0, minute);
	const hs = fixedValues(1, hour);
	// 時・分とも決まった値で組み合わせが少なければ「9:00・18:30」のように時刻で書く
	if (ms && hs && ms.length * hs.length <= 6)
		return hs.flatMap((h) => ms.map((m) => `${h}:${pad(m)}`)).join("・");
	const minuteParts = partLabels(0, minute);
	const minuteText = isEvery(minute) ? "毎分" : minuteParts.join("・");
	if (!isEvery(hour)) return `${partLabels(1, hour).join("・")} ${minuteText}`;
	if (isEvery(minute)) return "毎分";
	// 「30分ごと」だけなら「毎時」は付けない
	if (minuteParts.length === 1 && minute.intervals.length === 1)
		return minuteText;
	return `毎時 ${minuteText}`;
}

/** 実行タイミングの説明文（例: 「毎週 月曜日 9:00 に実行」）。 */
export function describeCron(state: CronState): string {
	const [minute, hour, dom, month, dow] = state;
	const monthText = isEvery(month) ? "" : partLabels(3, month).join("・");
	let domText = isEvery(dom) ? "" : partLabels(2, dom).join("・");
	let dowText = isEvery(dow) ? "" : partLabels(4, dow).join("・");
	if (!monthText && domText) domText = `毎月 ${domText}`;
	if (!monthText && dowText) dowText = `毎週 ${dowText}`;
	// 日と曜日は OR で評価されるので「または」でつなぐ
	const dayText = [domText, dowText].filter(Boolean).join(" または ");
	let date = [monthText, dayText].filter(Boolean).join(" ");
	if (monthText && !dayText) date = `${monthText}の毎日`;
	// 毎日決まった時に動くときだけ「毎日」を付ける（「毎時」「2時間ごと」には付けない）
	if (!date && fixedValues(1, hour)) date = "毎日";
	return `${date ? `${date} ` : ""}${timeText(minute, hour)} に実行`;
}

/** 日と曜日を両方指定しているか（どちらか一方が一致した日に実行される）。 */
export function hasDomAndDow(state: CronState): boolean {
	return !isEvery(state[2]) && !isEvery(state[4]);
}
