/**
 * エージェントの返答（Markdown）を、読み上げ用の文の列に変える。
 *
 * - コードブロック・表・URL は読み上げず、画面を見てもらう一言に置き換える。
 * - 絵文字は Irodori-TTS が演出として解釈するもの（{@link ANNOTATION_EMOJIS}）だけ残す。
 * - 文は句点などで区切る。最初の文は短く切って、最初の音を早く出す（長い文ほど生成に時間がかかる）。
 */
import { ANNOTATION_EMOJIS } from "./tts/text";

const ALLOWED_EMOJI = new Set(ANNOTATION_EMOJIS);
/** 1 文の上限（文字）。超えたら読点で分ける（長すぎる文は長さ予測と生成が不安定になる）。 */
export const MAX_SENTENCE_CHARS = 80;
/** 最初の文の上限。最初の音が鳴るまでの時間を短くする。 */
export const FIRST_SENTENCE_CHARS = 24;
/** これより短い文は次の文とつなげる（短すぎると 1 文ごとの固定時間が目立つ）。 */
const MIN_SENTENCE_CHARS = 6;

/** Markdown の記法を外して、読み上げる平文にする。 */
export function speakableText(markdown: string): string {
	let text = markdown.replace(/\r\n?/g, "\n");
	text = text.replace(
		/```[\s\S]*?(```|$)/g,
		"\n（コードは画面で確認してください）\n",
	);
	// 表（| で始まる行が続く）は読み上げない。
	text = text.replace(
		/(^\|.*\|\s*$\n?)+/gm,
		"\n（表は画面で確認してください）\n",
	);
	text = text.replace(/!\[[^\]]*\]\([^)]*\)/g, "");
	text = text.replace(/\[([^\]]+)\]\((?:[^)]*)\)/g, "$1");
	text = text.replace(/<https?:\/\/[^>]+>|https?:\/\/[^\s)）」』]+/g, "リンク");
	text = text.replace(/<[^>]+>/g, "");
	text = text.replace(/`([^`]*)`/g, "$1");
	text = text.replace(/^\s{0,3}#{1,6}\s+/gm, "");
	text = text.replace(/^\s*>\s?/gm, "");
	text = text.replace(/^\s*(?:[-*+]|\d+[.)])\s+(?:\[[ xX]\]\s+)?/gm, "");
	text = text.replace(/^\s*(?:-{3,}|\*{3,}|_{3,})\s*$/gm, "");
	text = text.replace(/(\*\*|__|\*|_|~~)(?=\S)([\s\S]*?\S)\1/g, "$2");
	text = text.replace(
		/\p{Extended_Pictographic}(?:️|‍\p{Extended_Pictographic}️?)*/gu,
		(emoji) => (ALLOWED_EMOJI.has(emoji) ? emoji : ""),
	);
	return text
		.replace(/[ \t]+/g, " ")
		.replace(/\n{2,}/g, "\n")
		.trim();
}

/** 長い文を読点（なければ空白）の位置で、上限を超えない範囲でなるべく後ろで分ける。 */
function splitLong(sentence: string, max: number): string[] {
	const out: string[] = [];
	let rest = sentence;
	while ([...rest].length > max) {
		const chars = [...rest];
		const head = chars.slice(0, max).join("");
		let cut = Math.max(
			head.lastIndexOf("、"),
			head.lastIndexOf("，"),
			head.lastIndexOf(","),
		);
		if (cut < max / 3) cut = head.lastIndexOf(" ");
		const at = cut >= max / 3 ? cut + 1 : head.length;
		out.push(rest.slice(0, at).trim());
		rest = rest.slice(at).trim();
	}
	if (rest) out.push(rest);
	return out;
}

/** 読み上げる文の列。空なら読み上げるものが無い。 */
export function speechSentences(markdown: string): string[] {
	const text = speakableText(markdown);
	const raw = text
		.split(/(?<=[。．！？!?♪…\n])/u)
		.map((s) => s.replace(/\n/g, " ").trim())
		.filter(Boolean);
	const merged: string[] = [];
	for (const sentence of raw) {
		const last = merged.at(-1);
		if (
			last !== undefined &&
			[...last].length < MIN_SENTENCE_CHARS &&
			[...last].length + [...sentence].length <= MAX_SENTENCE_CHARS
		)
			merged[merged.length - 1] = `${last}${sentence}`;
		else merged.push(sentence);
	}
	const sentences = merged.flatMap((s) => splitLong(s, MAX_SENTENCE_CHARS));
	if (sentences.length > 0) sentences.splice(0, 1, ...splitFirst(sentences[0]));
	return sentences;
}

/** 最初の文が長ければ、先頭の読点で 1 回だけ分ける（読点が無ければそのまま）。 */
function splitFirst(sentence: string): string[] {
	const chars = [...sentence];
	if (chars.length <= FIRST_SENTENCE_CHARS) return [sentence];
	const head = chars.slice(0, FIRST_SENTENCE_CHARS).join("");
	const cut = Math.max(
		head.lastIndexOf("、"),
		head.lastIndexOf("，"),
		head.lastIndexOf(","),
	);
	if (cut < MIN_SENTENCE_CHARS) return [sentence];
	return [
		sentence.slice(0, cut + 1).trim(),
		sentence.slice(cut + 1).trim(),
	].filter(Boolean);
}
