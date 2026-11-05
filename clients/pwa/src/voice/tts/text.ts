/**
 * Irodori-TTS v4.1 の入力テキスト処理（ブラウザ版）。上流 `irodori_tts` の次の移植:
 * - `text_normalization.normalize_text`（読み上げテキストの正規化）
 * - modernbert-ja の Unigram トークナイザ（Metaspace・byte fallback。`tokenizers` の Viterbi と同じ結果）
 * - `token_split.RareTokenSplitter`（読み上げテキストだけ、まれな長いトークンを無効化して短く分割）
 * - `duration.build_duration_features`（長さ予測の補助特徴）
 *
 * 上流のランタイムは読み上げテキストに rare-token split をかけ、キャプションにはかけない
 * （正規化もしない）。ここでも同じ扱いにする（`createTokenizers`）。
 */

const SIMPLE_REPLACE: [string, string][] = [
	["\t", ""],
	["[n]", ""],
	["　", ""],
	["？", "?"],
	["！", "!"],
	["♥", "♡"],
	["●", "○"],
	["◯", "○"],
	["〇", "○"],
];
const REGEX_REPLACE: [RegExp, string][] = [
	[/[;▼♀♂《》≪≫①②③④⑤⑥]/gu, ""],
	[/[˗‐-―⁃−⎯⏤─━⸺⸻]/gu, ""],
	[/[～〜]/gu, "ー"],
	[/…{3,}/gu, "……"],
];
const BRACKETS: Record<string, string> = {
	"「": "」",
	"『": "』",
	"（": "）",
	"【": "】",
	"(": ")",
};

function stripOuterBrackets(text: string): string {
	for (;;) {
		const chars = [...text];
		if (chars.length < 2) return text;
		const start = chars[0];
		const end = chars[chars.length - 1];
		if (BRACKETS[start] !== end) return text;
		let depth = 0;
		let enclosing = true;
		for (let i = 0; i < chars.length; i++) {
			if (chars[i] === start) depth++;
			else if (chars[i] === end) depth--;
			if (depth === 0 && i < chars.length - 1) {
				enclosing = false;
				break;
			}
		}
		if (!(enclosing && depth === 0)) return text;
		text = chars.slice(1, -1).join("");
	}
}

export function normalizeText(text: string): string {
	for (const [from, to] of SIMPLE_REPLACE) text = text.split(from).join(to);
	for (const [re, to] of REGEX_REPLACE) text = text.replace(re, to);
	text = stripOuterBrackets(text).normalize("NFKC");
	return text.split("...").join("…").split("..").join("…");
}

/** `tokenizer.json` のうち使う部分（Unigram・Metaspace `prepend_scheme: never`・normalizer なし）。 */
export type TokenizerJson = {
	model: {
		type: "Unigram";
		unk_id: number;
		byte_fallback?: boolean;
		vocab: [string, number][];
	};
};
/** モデル同梱の `irodori_tokenizer.json`（エクスポート時に上流の RareTokenSplitter から生成）。 */
export type IrodoriTokenizerExtra = {
	bos_token_id: number;
	disabled: number[];
	forced: Record<string, number[]>;
};

type TrieNode = { next: Map<string, TrieNode>; id: number };
/** `tokenizers` の Unigram が未知文字に付けるスコア（最小スコア − 10）。 */
const UNK_PENALTY = 10;

/** modernbert-ja の Unigram トークナイザ。BOS などの特殊トークンは付けない。 */
export class UnigramTokenizer {
	private readonly root: TrieNode = { next: new Map(), id: -1 };
	private readonly scores: Float64Array;
	private readonly unkId: number;
	private readonly unkScore: number;
	private readonly byteFallback: boolean;
	private readonly byteIds = new Map<number, number>();

	constructor(json: TokenizerJson, disabled: Iterable<number> = []) {
		const { vocab, unk_id: unkId, byte_fallback: byteFallback } = json.model;
		this.unkId = unkId;
		this.scores = Float64Array.from(vocab, ([, score]) => score);
		for (const id of disabled) this.scores[id] = -1e9;
		let min = Infinity;
		vocab.forEach(([piece], id) => {
			min = Math.min(min, this.scores[id]);
			const byte = /^<0x([0-9A-F]{2})>$/.exec(piece);
			if (byteFallback && byte) {
				this.byteIds.set(parseInt(byte[1], 16), id);
				return;
			}
			let node = this.root;
			for (const ch of piece) {
				let child = node.next.get(ch);
				if (!child) {
					child = { next: new Map(), id: -1 };
					node.next.set(ch, child);
				}
				node = child;
			}
			node.id = id;
		});
		this.unkScore = min - UNK_PENALTY;
		this.byteFallback = Boolean(byteFallback);
	}

	encode(text: string): number[] {
		// Metaspace（replacement "▁"・prepend なし・split なし）: 空白を置き換えるだけ。
		const chars = [...text.replaceAll(" ", "▁")];
		const n = chars.length;
		if (n === 0) return [];
		// Viterbi: best[i] は先頭 i 文字の最良分割（スコア・直前ノード）。
		const bestScore = new Float64Array(n + 1).fill(-Infinity);
		const bestFrom = new Int32Array(n + 1).fill(-1);
		const bestId = new Int32Array(n + 1).fill(-1);
		bestScore[0] = 0;
		for (let start = 0; start < n; start++) {
			if (bestScore[start] === -Infinity) continue;
			let node: TrieNode | undefined = this.root;
			let hasSingle = false;
			for (let end = start; end < n && node; end++) {
				node = node.next.get(chars[end]);
				if (!node) break;
				if (node.id < 0) continue;
				if (end === start) hasSingle = true;
				const score = bestScore[start] + this.scores[node.id];
				if (bestFrom[end + 1] < 0 || score > bestScore[end + 1]) {
					bestScore[end + 1] = score;
					bestFrom[end + 1] = start;
					bestId[end + 1] = node.id;
				}
			}
			if (!hasSingle) {
				const score = bestScore[start] + this.unkScore;
				if (bestFrom[start + 1] < 0 || score > bestScore[start + 1]) {
					bestScore[start + 1] = score;
					bestFrom[start + 1] = start;
					bestId[start + 1] = this.unkId;
				}
			}
		}
		const pieces: { id: number; from: number; to: number }[] = [];
		for (let end = n; end > 0; end = bestFrom[end])
			pieces.push({ id: bestId[end], from: bestFrom[end], to: end });
		pieces.reverse();
		const ids: number[] = [];
		for (let i = 0; i < pieces.length; i++) {
			if (pieces[i].id !== this.unkId || !this.byteFallback) {
				ids.push(pieces[i].id);
				continue;
			}
			// 連続する未知文字はまとめて UTF-8 のバイト列トークンにする（無いバイトがあれば unk 1 つ）。
			const from = pieces[i].from;
			while (i + 1 < pieces.length && pieces[i + 1].id === this.unkId) i++;
			const bytes = new TextEncoder().encode(
				chars.slice(from, pieces[i].to).join(""),
			);
			const byteIds = Array.from(bytes, (b) => this.byteIds.get(b));
			if (byteIds.every((id) => id !== undefined))
				ids.push(...(byteIds as number[]));
			else ids.push(this.unkId);
		}
		return ids;
	}
}

export class IrodoriTokenizer {
	constructor(
		private readonly unigram: UnigramTokenizer,
		private readonly bos: number,
		private readonly forced = new Map<number, number[]>(),
	) {}

	/** BOS なしの本文 id（読み上げ用は `RareTokenSplitter.encode` と同じ結果）。 */
	encodeBody(text: string): number[] {
		return this.unigram
			.encode(text)
			.flatMap((id) => this.forced.get(id) ?? [id]);
	}

	/** BOS を先頭に付け、`maxLen` で切った id とマスク。 */
	encode(
		text: string,
		maxLen: number,
	): { ids: BigInt64Array; mask: Uint8Array } {
		const ids = [this.bos, ...this.encodeBody(text).slice(0, maxLen - 1)];
		return {
			ids: BigInt64Array.from(ids, (id) => BigInt(id)),
			mask: new Uint8Array(ids.length).fill(1),
		};
	}
}

export function createTokenizers(
	json: TokenizerJson,
	extra: IrodoriTokenizerExtra,
) {
	const forced = new Map(
		Object.entries(extra.forced).map(([k, v]) => [Number(k), v]),
	);
	return {
		text: new IrodoriTokenizer(
			new UnigramTokenizer(json, extra.disabled),
			extra.bos_token_id,
			forced,
		),
		caption: new IrodoriTokenizer(
			new UnigramTokenizer(json),
			extra.bos_token_id,
		),
	};
}
export type Tokenizers = ReturnType<typeof createTokenizers>;

/** 長さ予測が理解する演出用の絵文字（上流 `ALLOWED_ANNOTATION_EMOJIS`）。 */
export const ANNOTATION_EMOJIS = [
	"⏩",
	"⏱️",
	"⏸️",
	"🌬️",
	"🍭",
	"🎛️",
	"🎭",
	"🎵",
	"🐢",
	"🐱",
	"👂",
	"👃",
	"👅",
	"👌",
	"👏",
	"💋",
	"💥",
	"💦",
	"💪",
	"📄",
	"📞",
	"📢",
	"📣",
	"😆",
	"😊",
	"😌",
	"😎",
	"😏",
	"😒",
	"😖",
	"😟",
	"😠",
	"😪",
	"😭",
	"😮",
	"😮‍💨",
	"😰",
	"😱",
	"😲",
	"😴",
	"🙄",
	"🙏",
	"🤐",
	"🤔",
	"🤢",
	"🤧",
	"🤭",
	"🥤",
	"🥱",
	"🥴",
	"🥵",
	"🥹",
	"🥺",
	"🫣",
	"🫶",
	"📖",
];
const EMOJI_RE = new RegExp(
	[...ANNOTATION_EMOJIS]
		.sort((a, b) => b.length - a.length)
		.map((e) => e.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"))
		.join("|"),
	"gu",
);
const log1pCap = (v: number, cap: number) =>
	Math.log1p(Math.min(Math.max(v, 0), cap)) / Math.log1p(cap);
const count = (text: string, ...subs: string[]) =>
	subs.reduce((n, s) => n + text.split(s).length - 1, 0);

/** `build_duration_features` の 1 行分。文字数は Python の `len()` と同じくコードポイントで数える。 */
export function durationFeatures(
	text: string,
	tokenCount: number,
	maxTextLen: number,
	hasSpeaker: boolean,
): Float32Array {
	const chars = [...text];
	const charCount = Math.max(chars.length, 1);
	let kana = 0,
		kanji = 0,
		alnum = 0;
	for (const ch of chars) {
		const c = ch.codePointAt(0) ?? 0;
		if ((c >= 0x3040 && c <= 0x309f) || (c >= 0x30a0 && c <= 0x30ff)) kana++;
		if (
			(c >= 0x3400 && c <= 0x4dbf) ||
			(c >= 0x4e00 && c <= 0x9fff) ||
			(c >= 0xf900 && c <= 0xfaff) ||
			(c >= 0x20000 && c <= 0x2fa1f)
		)
			kanji++;
		if (/^[A-Za-z0-9]$/.test(ch)) alnum++;
	}
	return Float32Array.from([
		Math.min(Math.max(tokenCount, 0), maxTextLen) / maxTextLen,
		log1pCap(charCount, 512),
		tokenCount / charCount,
		log1pCap(count(text, "。", "."), 8),
		log1pCap(count(text, "、", ","), 16),
		log1pCap(count(text, "ー"), 8),
		log1pCap(count(text, "…"), 8),
		log1pCap(count(text, "！", "!"), 8),
		log1pCap(count(text, "？", "?"), 8),
		log1pCap((text.match(EMOJI_RE) ?? []).length, 8),
		kana / charCount,
		kanji / charCount,
		alnum / charCount,
		hasSpeaker ? 1 : 0,
	]);
}
