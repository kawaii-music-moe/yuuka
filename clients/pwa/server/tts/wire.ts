/** サーバーでの音声生成の入出力（検証と変換）。PWA 側（`src/voice/serverTts.ts`）と形を合わせる。 */
import type { SpeakerCondition } from "../../src/voice/tts/pipeline";

const SPEAKER_DIM = 768;
const MAX_TEXT_CHARS = 400;

export class ServerTtsError extends Error {
	constructor(
		public readonly status: number,
		message: string,
	) {
		super(message);
	}
}

export type SpeakRequest = {
	text: string;
	voice:
		| { kind: "design"; caption: string; voiceSeed: number }
		| {
				kind: "clone";
				caption: string;
				key: string;
				stateBase64?: string;
				mask?: boolean[];
		  };
	seed: number;
	durationScale: number;
};

export function parseSpeakRequest(raw: unknown): SpeakRequest {
	const body = raw as Partial<SpeakRequest> & {
		voice?: Record<string, unknown>;
	};
	if (typeof body.text !== "string" || [...body.text].length > MAX_TEXT_CHARS)
		throw new ServerTtsError(
			400,
			`text は ${MAX_TEXT_CHARS} 文字以内の文字列にしてください`,
		);
	const voice = body.voice;
	const caption = typeof voice?.caption === "string" ? voice.caption : "";
	const seed = Number.isFinite(body.seed) ? Number(body.seed) : 0;
	const durationScale =
		typeof body.durationScale === "number" &&
		body.durationScale >= 0.5 &&
		body.durationScale <= 2
			? body.durationScale
			: 1;
	if (voice?.kind === "design" && typeof voice.voiceSeed === "number")
		return {
			text: body.text,
			voice: { kind: "design", caption, voiceSeed: voice.voiceSeed },
			seed,
			durationScale,
		};
	if (voice?.kind === "clone" && typeof voice.key === "string" && voice.key) {
		const stateBase64 =
			typeof voice.stateBase64 === "string" ? voice.stateBase64 : undefined;
		const mask = Array.isArray(voice.mask)
			? voice.mask.map(Boolean)
			: undefined;
		return {
			text: body.text,
			voice: { kind: "clone", caption, key: voice.key, stateBase64, mask },
			seed,
			durationScale,
		};
	}
	throw new ServerTtsError(400, "voice が不正です");
}

export function decodeSpeaker(
	stateBase64: string,
	mask: boolean[],
): SpeakerCondition {
	const bytes = Buffer.from(stateBase64, "base64");
	if (mask.length === 0 || bytes.length !== mask.length * SPEAKER_DIM * 4)
		throw new ServerTtsError(400, "声のデータの大きさが mask と合いません");
	return {
		state: new Float32Array(
			bytes.buffer,
			bytes.byteOffset,
			bytes.length / 4,
		).slice(),
		mask,
	};
}

export function encodeSpeaker(speaker: SpeakerCondition): {
	stateBase64: string;
	mask: boolean[];
} {
	return {
		stateBase64: Buffer.from(
			speaker.state.buffer,
			speaker.state.byteOffset,
			speaker.state.byteLength,
		).toString("base64"),
		mask: speaker.mask,
	};
}

/** [-1, 1] の float を 16 bit PCM（リトルエンディアン）にする。 */
export function floatToPcm16(audio: Float32Array): Buffer {
	const out = Buffer.alloc(audio.length * 2);
	for (let i = 0; i < audio.length; i++)
		out.writeInt16LE(
			Math.round(Math.max(-1, Math.min(1, audio[i])) * 0x7fff),
			i * 2,
		);
	return out;
}

export function pcm16ToFloat(bytes: Buffer): Float32Array {
	const out = new Float32Array(Math.floor(bytes.length / 2));
	for (let i = 0; i < out.length; i++)
		out[i] = bytes.readInt16LE(i * 2) / 0x8000;
	return out;
}
