/**
 * 声のサンプル（参照音声）の前処理。上流 `DACVAECodec.encode_waveform` の参照音声の扱いに合わせる:
 * モノラル化 → 48 kHz → 長さの上限で切る → 積分ラウドネスを -16 LUFS にそろえる → ピークを 1 以下にする。
 *
 * ラウドネスは audiotools（pyloudnorm 由来）の ITU-R BS.1770-4 実装の移植: K 特性フィルタ（2 段の
 * 双二次）、0.4 秒ブロック・75% 重ね、-70 LUFS の絶対ゲートと -10 LU の相対ゲート、0.5 秒未満は
 * 0 埋めして測る、下限 -70。
 */

export const REFERENCE_SAMPLE_RATE = 48000;
/** 上流 v4.1 の推奨上限（checkpoint の `ref_max_seconds`）。 */
export const MAX_REFERENCE_SECONDS = 120;
/** 上流ランタイムの既定（`ref_normalize_db`）。 */
const TARGET_LUFS = -16;
const MIN_LOUDNESS = -70;

// pyloudnorm の K 特性フィルタ（48 kHz）。[b0, b1, b2], [a0, a1, a2]
const K_WEIGHTING_48K: [number[], number[]][] = [
	[
		[1.5351828863637502, -2.691804030199196, 1.198426263333146],
		[1, -1.6906995865986896, 0.7325047060963897],
	],
	[
		[0.9950442970178917, -1.9900885940357833, 0.9950442970178917],
		[1, -1.990076284018423, 0.9901009040531438],
	],
];

function biquad(x: Float32Array, [b, a]: [number[], number[]]): Float32Array {
	const y = new Float32Array(x.length);
	let x1 = 0,
		x2 = 0,
		y1 = 0,
		y2 = 0;
	for (let i = 0; i < x.length; i++) {
		const v = b[0] * x[i] + b[1] * x1 + b[2] * x2 - a[1] * y1 - a[2] * y2;
		x2 = x1;
		x1 = x[i];
		y2 = y1;
		y1 = v;
		y[i] = v;
	}
	return y;
}

/** モノラル 48 kHz の積分ラウドネス（LUFS）。 */
export function integratedLoudness(
	samples: Float32Array,
	rate = REFERENCE_SAMPLE_RATE,
): number {
	if (rate !== REFERENCE_SAMPLE_RATE)
		throw new Error("integratedLoudness expects 48 kHz audio");
	let data = samples;
	if (data.length < rate * 0.5) {
		// audiotools: pad_len = int((0.5 - duration) * rate)
		const padded = new Float32Array(
			data.length + Math.floor((0.5 - data.length / rate) * rate),
		);
		padded.set(data);
		data = padded;
	}
	const filtered = K_WEIGHTING_48K.reduce(biquad, data);
	const kernel = Math.floor(0.4 * rate);
	const stride = Math.floor(0.4 * rate * 0.25);
	// julius.core.unfold: 末尾を 0 埋めして最後のブロックまで取る。
	const blocks =
		Math.ceil((Math.max(filtered.length, kernel) - kernel) / stride) + 1;
	const z = new Float64Array(blocks);
	for (let j = 0; j < blocks; j++) {
		let sum = 0;
		const end = Math.min(j * stride + kernel, filtered.length);
		for (let i = j * stride; i < end; i++) sum += filtered[i] * filtered[i];
		z[j] = sum / kernel;
	}
	const loud = (v: number) => -0.691 + 10 * Math.log10(v);
	const gatedMean = (threshold: number) => {
		let sum = 0,
			n = 0;
		for (const v of z)
			if (loud(v) > threshold && loud(v) > -70) {
				sum += v;
				n++;
			}
		return n > 0 ? sum / n : 0;
	};
	const relative = loud(gatedMean(-70)) - 10;
	const lufs = loud(gatedMean(relative));
	return Number.isFinite(lufs) ? Math.max(lufs, MIN_LOUDNESS) : MIN_LOUDNESS;
}

/** 複数チャンネルを平均してモノラルにする（上流は `mean(dim=channel)`）。 */
export function downmix(channels: Float32Array[]): Float32Array {
	if (channels.length === 1) return channels[0];
	const out = new Float32Array(channels[0].length);
	for (const ch of channels)
		for (let i = 0; i < out.length; i++) out[i] += ch[i] / channels.length;
	return out;
}

/** 48 kHz モノラルの参照音声を、エンコーダに渡す形にする（長さの上限・ラウドネス・ピーク）。 */
export function prepareReference(mono48k: Float32Array): Float32Array {
	const trimmed =
		mono48k.length > MAX_REFERENCE_SECONDS * REFERENCE_SAMPLE_RATE
			? mono48k.slice(0, MAX_REFERENCE_SECONDS * REFERENCE_SAMPLE_RATE)
			: mono48k;
	const gain = 10 ** ((TARGET_LUFS - integratedLoudness(trimmed)) / 20);
	const out = new Float32Array(trimmed.length);
	let peak = 0;
	for (let i = 0; i < out.length; i++) {
		out[i] = trimmed[i] * gain;
		peak = Math.max(peak, Math.abs(out[i]));
	}
	if (peak > 1) for (let i = 0; i < out.length; i++) out[i] /= peak;
	return out;
}
