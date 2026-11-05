/** 音声合成 Worker の窓口（メインスレッド側）。Worker は 1 つだけ作って使い回す。 */
import type { SpeakerCondition, StageTimings } from "./pipeline";
import type { VoiceSpec, WorkerRequest, WorkerResponse } from "./protocol";

export type ProbeResult = Extract<WorkerResponse, { type: "probe" }>;
export type ChunkHandler = (chunk: {
	audio: Float32Array;
	sampleRate: number;
	final: boolean;
	timings?: StageTimings;
}) => void;

type Pending = {
	onChunk: ChunkHandler;
	resolve: () => void;
	reject: (error: Error) => void;
};

export class IrodoriClient {
	private readonly worker: Worker;
	private nextId = 1;
	private readonly pending = new Map<number, Pending>();
	private readonly speakers = new Map<
		number,
		{
			resolve: (speaker: SpeakerCondition) => void;
			reject: (error: Error) => void;
		}
	>();
	// 画面を開き直す・素早く行き来するなどで、同じ要求が重なることがある。どれも取りこぼさないよう配列で待つ。
	/** probe は 1 要求につき 1 応答なので、届いた順に返す。 */
	private readonly probeWaiters: ((result: ProbeResult) => void)[] = [];
	/** 読み込みは 1 つしか走らないので、ready / エラーで待っている全員に返す。 */
	private loadWaiters: {
		resolve: (adapter: string) => void;
		reject: (error: Error) => void;
		onProgress: (loaded: number, total: number) => void;
	}[] = [];
	private clearWaiters: (() => void)[] = [];

	/** `worker` はテスト用に差し替えられる。 */
	constructor(worker?: Worker) {
		this.worker =
			worker ??
			new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
		this.worker.onmessage = (event: MessageEvent<WorkerResponse>) =>
			this.handle(event.data);
	}

	private send(message: WorkerRequest) {
		this.worker.postMessage(message);
	}

	private handle(message: WorkerResponse) {
		switch (message.type) {
			case "probe":
				this.probeWaiters.shift()?.(message);
				break;
			case "progress":
				this.loadWaiters.forEach((w) => {
					w.onProgress(message.loaded, message.total);
				});
				break;
			case "ready":
				this.takeLoadWaiters().forEach((w) => {
					w.resolve(message.adapter);
				});
				break;
			case "chunk":
				this.pending.get(message.id)?.onChunk(message);
				break;
			case "done":
				this.pending.get(message.id)?.resolve();
				this.pending.delete(message.id);
				break;
			case "speaker":
				this.speakers
					.get(message.id)
					?.resolve({ state: message.state, mask: message.mask });
				this.speakers.delete(message.id);
				break;
			case "cleared":
				this.clearWaiters.shift()?.();
				break;
			case "error":
				if (message.id !== undefined) {
					this.pending.get(message.id)?.reject(new Error(message.message));
					this.pending.delete(message.id);
					this.speakers.get(message.id)?.reject(new Error(message.message));
					this.speakers.delete(message.id);
				} else {
					this.takeLoadWaiters().forEach((w) => {
						w.reject(new Error(message.message));
					});
				}
				break;
		}
	}

	private takeLoadWaiters() {
		const waiters = this.loadWaiters;
		this.loadWaiters = [];
		return waiters;
	}

	/** WebGPU 対応かどうかと、モデルが保存済みかを調べる。 */
	probe(): Promise<ProbeResult> {
		return new Promise((resolve) => {
			this.probeWaiters.push(resolve);
			this.send({ type: "probe" });
		});
	}

	/** モデルを読み込む（未保存なら取得する）。読み込み済みならすぐに、GPU の名前を返す。 */
	load(onProgress: (loaded: number, total: number) => void): Promise<string> {
		return new Promise((resolve, reject) => {
			this.loadWaiters.push({ resolve, reject, onProgress });
			this.send({ type: "load" });
		});
	}

	/**
	 * 1 文を合成する。区切った音声が届くたびに `onChunk` を呼び、合成が終わったら解決する。
	 * `seed` は文ごとの抑揚の揺らぎ、`durationScale` は話す速さ（大きいほどゆっくり）。
	 */
	speak(
		text: string,
		voice: VoiceSpec,
		options: { seed: number; durationScale: number },
		onChunk: ChunkHandler,
	): Promise<void> {
		const id = this.nextId++;
		return new Promise((resolve, reject) => {
			this.pending.set(id, { onChunk, resolve, reject });
			this.send({ type: "speak", id, text, voice, ...options });
		});
	}

	/** 声を先に作っておく（最初の返答で声づくりの分だけ待たせないため）。 */
	prepareVoice(voice: VoiceSpec): Promise<void> {
		return this.speak(
			"",
			voice,
			{ seed: 0, durationScale: 1 },
			() => undefined,
		);
	}

	/** 声のサンプル（48 kHz モノラル）から話者条件を作る。初回は専用のエンコーダ（約 55 MB）を取得する。 */
	clone(audio: Float32Array): Promise<SpeakerCondition> {
		return this.speakerRequest((id) =>
			this.worker.postMessage(
				{ type: "clone", id, audio } satisfies WorkerRequest,
				[audio.buffer],
			),
		);
	}

	/** キャプションと声の番号から作る声の話者条件（保存用）。 */
	design(caption: string, voiceSeed: number): Promise<SpeakerCondition> {
		return this.speakerRequest((id) =>
			this.send({ type: "design", id, caption, voiceSeed }),
		);
	}

	private speakerRequest(
		start: (id: number) => void,
	): Promise<SpeakerCondition> {
		const id = this.nextId++;
		return new Promise((resolve, reject) => {
			this.speakers.set(id, { resolve, reject });
			start(id);
		});
	}

	/** 合成中・待ち中の文をすべて取りやめる（それぞれ `done` で解決される）。 */
	cancel() {
		this.send({ type: "cancel" });
	}

	clearStored(): Promise<void> {
		return new Promise((resolve) => {
			this.clearWaiters.push(resolve);
			this.send({ type: "clear" });
		});
	}
}

let shared: IrodoriClient | undefined;
/** 画面を離れても読み込んだモデルを保てるよう、Worker はアプリ全体で 1 つにする。 */
export function irodoriClient(): IrodoriClient {
	shared ??= new IrodoriClient();
	return shared;
}
