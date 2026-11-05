/**
 * 返答の読み上げ。この端末で Irodori-TTS を動かせればそれを使い（{@link IrodoriSpeaker}）、
 * 動かせなければブラウザ標準の読み上げ（{@link BrowserSpeaker}）にする。
 */
import type { IrodoriClient } from "./tts/client";
import type { StageTimings } from "./tts/pipeline";
import type { VoiceSpec } from "./tts/protocol";

export type SpeechStats = {
	firstAudioMs: number;
	stallMs: number;
	audioSec: number;
	sentences: { text: string; audioSec: number; timings?: StageTimings }[];
};

export interface ReplySpeaker {
	/** 文の列を順に読み上げ、読み終える（または止められる）と解決する。 */
	speak(sentences: string[]): Promise<SpeechStats | undefined>;
	stop(): void;
}

/** 区切って届く音声を、隙間なく続けて鳴らす。 */
export class ChunkPlayer {
	private ctx?: AudioContext;
	private nextStart = 0;
	private readonly sources = new Set<AudioBufferSourceNode>();
	private idleWaiters: (() => void)[] = [];
	/** 再生が途切れた（次の音が間に合わなかった）合計秒数。 */
	stallSec = 0;

	/** ユーザー操作の中で呼ぶ（自動再生の制限で、操作なしに作った AudioContext は鳴らない）。 */
	async unlock() {
		this.ctx ??= new AudioContext({ sampleRate: 48000 });
		if (this.ctx.state !== "running") await this.ctx.resume();
	}

	enqueue(audio: Float32Array, sampleRate: number) {
		const ctx = this.ctx;
		if (!ctx || audio.length === 0) return;
		const buffer = ctx.createBuffer(1, audio.length, sampleRate);
		buffer.copyToChannel(audio as Float32Array<ArrayBuffer>, 0);
		const source = ctx.createBufferSource();
		source.buffer = buffer;
		source.connect(ctx.destination);
		const now = ctx.currentTime + 0.02;
		if (this.sources.size > 0 && now > this.nextStart)
			this.stallSec += now - this.nextStart;
		const start = Math.max(now, this.nextStart);
		source.start(start);
		this.nextStart = start + buffer.duration;
		this.sources.add(source);
		source.onended = () => {
			this.sources.delete(source);
			if (this.sources.size === 0) this.flushIdle();
		};
	}

	get playing() {
		return this.sources.size > 0;
	}

	/** 今鳴っている・予約済みの音を鳴らし終えたら解決する。 */
	whenIdle(): Promise<void> {
		if (!this.playing) return Promise.resolve();
		return new Promise((resolve) => this.idleWaiters.push(resolve));
	}

	stop() {
		for (const source of this.sources) {
			source.onended = null;
			try {
				source.stop();
			} catch {
				/* 開始前でも止められる */
			}
		}
		this.sources.clear();
		this.nextStart = 0;
		this.flushIdle();
	}

	private flushIdle() {
		const waiters = this.idleWaiters;
		this.idleWaiters = [];
		waiters.forEach((resolve) => {
			resolve();
		});
	}
}

export class IrodoriSpeaker implements ReplySpeaker {
	private stopped = false;

	constructor(
		private readonly client: IrodoriClient,
		private readonly player: ChunkPlayer,
		private readonly voice: () => { voice: VoiceSpec; durationScale: number },
	) {}

	async speak(sentences: string[]): Promise<SpeechStats | undefined> {
		this.stopped = false;
		this.player.stallSec = 0;
		const started = performance.now();
		let firstAudioMs = -1;
		const stats: SpeechStats["sentences"] = sentences.map((text) => ({
			text,
			audioSec: 0,
		}));
		// 全文をまとめて依頼する（Worker は 1 件ずつ順に作り、前の文の再生中に次の文を作る）。
		// 声は全文で同じ。文ごとに変えるのは抑揚の揺らぎ（seed）だけ。
		const { voice, durationScale } = this.voice();
		const jobs = sentences.map((text, i) =>
			this.client.speak(text, voice, { seed: i, durationScale }, (chunk) => {
				if (this.stopped) return;
				if (firstAudioMs < 0) firstAudioMs = performance.now() - started;
				stats[i].audioSec += chunk.audio.length / chunk.sampleRate;
				if (chunk.timings) stats[i].timings = chunk.timings;
				this.player.enqueue(chunk.audio, chunk.sampleRate);
			}),
		);
		const results = await Promise.allSettled(jobs);
		await this.player.whenIdle();
		const failed = results.find(
			(r): r is PromiseRejectedResult => r.status === "rejected",
		);
		if (failed && !this.stopped) throw failed.reason;
		if (this.stopped || firstAudioMs < 0) return undefined;
		return {
			firstAudioMs,
			stallMs: this.player.stallSec * 1000,
			audioSec: stats.reduce((a, s) => a + s.audioSec, 0),
			sentences: stats,
		};
	}

	stop() {
		this.stopped = true;
		this.client.cancel();
		this.player.stop();
	}
}

/** ブラウザ標準の読み上げ（WebGPU が使えない端末向け）。声は OS のもの。 */
export class BrowserSpeaker implements ReplySpeaker {
	private finish?: () => void;

	/** `rate` は読み上げの速さ（1 が標準）。 */
	constructor(private readonly rate: () => number = () => 1) {}

	static get available() {
		return typeof window !== "undefined" && "speechSynthesis" in window;
	}

	async speak(sentences: string[]): Promise<undefined> {
		const synth = window.speechSynthesis;
		synth.cancel();
		const voice = synth.getVoices().find((v) => v.lang.startsWith("ja"));
		await new Promise<void>((resolve) => {
			this.finish = resolve;
			if (sentences.length === 0) return resolve();
			sentences.forEach((text, i) => {
				const utterance = new SpeechSynthesisUtterance(text);
				utterance.lang = "ja-JP";
				utterance.rate = this.rate();
				if (voice) utterance.voice = voice;
				if (i === sentences.length - 1) {
					utterance.onend = () => resolve();
					utterance.onerror = () => resolve();
				}
				synth.speak(utterance);
			});
		});
		this.finish = undefined;
		return undefined;
	}

	stop() {
		window.speechSynthesis.cancel();
		this.finish?.();
	}
}
