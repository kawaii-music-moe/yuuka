/** 声のサンプルの取り込み（音声ファイルの読み込み・マイクでの録音）。どちらも 48 kHz モノラルにする。 */
import { downmix, REFERENCE_SAMPLE_RATE } from "./tts/referenceAudio";

/** 録音の上限（秒）。上流の推奨は合計 30 秒前後。 */
export const MAX_RECORDING_SECONDS = 30;
/** これより短いサンプルは声の特徴を取りにくい。 */
export const MIN_SAMPLE_SECONDS = 3;

export type VoiceSample = { audio: Float32Array; seconds: number };

/** 音声ファイル（wav / mp3 / m4a / ogg / webm など）を 48 kHz モノラルにする。 */
export async function decodeSample(blob: Blob): Promise<VoiceSample> {
	// OfflineAudioContext の decodeAudioData は、そのコンテキストのサンプルレートへ変換して返す。
	const ctx = new OfflineAudioContext(1, 1, REFERENCE_SAMPLE_RATE);
	let buffer: AudioBuffer;
	try {
		buffer = await ctx.decodeAudioData(await blob.arrayBuffer());
	} catch {
		throw new Error(
			"音声ファイルを読み込めませんでした（対応していない形式かもしれません）。",
		);
	}
	const channels = Array.from({ length: buffer.numberOfChannels }, (_, i) =>
		buffer.getChannelData(i),
	);
	const audio = downmix(channels).slice();
	return { audio, seconds: audio.length / REFERENCE_SAMPLE_RATE };
}

export type Recording = {
	/** 録音を止めて、48 kHz モノラルにした音声を返す。 */
	stop(): Promise<VoiceSample>;
	/** 録音を捨てる。 */
	cancel(): void;
};

/** マイクで録音を始める。`MAX_RECORDING_SECONDS` で自動的に止まり、`onAutoStop` が呼ばれる。 */
export async function startRecording(
	onAutoStop: () => void,
): Promise<Recording> {
	if (
		!navigator.mediaDevices?.getUserMedia ||
		typeof MediaRecorder === "undefined"
	)
		throw new Error("このブラウザは録音に対応していません。");
	let stream: MediaStream;
	try {
		// 音量はあとで揃えるので自動ゲインは切る。雑音抑制は声の特徴を取りやすくするために残す。
		stream = await navigator.mediaDevices.getUserMedia({
			audio: {
				channelCount: 1,
				autoGainControl: false,
				noiseSuppression: true,
				echoCancellation: true,
			},
		});
	} catch {
		throw new Error(
			"マイクを使えません。ブラウザの設定でマイクを許可してください。",
		);
	}
	const recorder = new MediaRecorder(stream);
	const chunks: Blob[] = [];
	recorder.ondataavailable = (event) => {
		if (event.data.size > 0) chunks.push(event.data);
	};
	const stopped = new Promise<Blob>((resolve) => {
		recorder.onstop = () => {
			stream.getTracks().forEach((track) => {
				track.stop();
			});
			resolve(new Blob(chunks, { type: recorder.mimeType }));
		};
	});
	const timer = setTimeout(() => {
		if (recorder.state === "recording") {
			recorder.stop();
			onAutoStop();
		}
	}, MAX_RECORDING_SECONDS * 1000);
	recorder.start();
	return {
		async stop() {
			clearTimeout(timer);
			if (recorder.state === "recording") recorder.stop();
			return decodeSample(await stopped);
		},
		cancel() {
			clearTimeout(timer);
			if (recorder.state === "recording") recorder.stop();
		},
	};
}
