import { computed, onScopeDispose, type Ref, ref } from "vue";
import { type EngineStatus, usesIrodori } from "./engineStatus";
import {
	ServerSpeaker,
	serverClone,
	serverDesign,
	serverTtsAvailable,
} from "./serverTts";
import {
	BrowserSpeaker,
	ChunkPlayer,
	IrodoriSpeaker,
	type ReplySpeaker,
} from "./speaker";
import { irodoriClient } from "./tts/client";
import type { SpeakerCondition } from "./tts/pipeline";
import type { VoiceSpec } from "./tts/protocol";

export type VoiceEngineOptions = {
	/** 今読み上げに使う声。 */
	voice: () => VoiceSpec;
	/** 話す速さ（duration_scale。大きいほどゆっくり）。 */
	speed: Ref<number>;
	/** この端末で作れても、サーバーで作る。 */
	useServer: Ref<boolean>;
};

/**
 * 返答を読み上げる仕組みを選ぶ。この端末で Irodori-TTS を動かせればこの端末で、無理ならサーバーで、
 * それも無ければブラウザ標準の読み上げを使う。
 */
export function useVoiceEngine(options: VoiceEngineOptions) {
	const status = ref<EngineStatus>({ kind: "checking" });
	/** サーバーでの音声生成が使えるか（設定されていて、動いているか）。 */
	const serverAvailable = ref(false);
	const canUseIrodori = computed(() => usesIrodori(status.value));

	const client = irodoriClient();
	const player = new ChunkPlayer();
	const synthesis = () => ({
		voice: options.voice(),
		durationScale: options.speed.value,
	});
	const onDevice = new IrodoriSpeaker(client, player, synthesis);
	const onServer = new ServerSpeaker(player, synthesis);
	const browser = BrowserSpeaker.available
		? new BrowserSpeaker(() => 1 / options.speed.value)
		: undefined;
	let disposed = false;
	onScopeDispose(() => {
		disposed = true;
	});

	/** 今読み上げに使う仕組み（どれも使えなければ undefined）。 */
	function speaker(): ReplySpeaker | undefined {
		if (status.value.kind === "ready") return onDevice;
		// この端末のモデルを準備している間も、サーバーが使えればそちらで読む。
		if (status.value.kind === "server" || serverAvailable.value)
			return onServer;
		return browser;
	}

	/** この端末で作れないときの行き先（サーバーが使えればサーバー、無ければ標準の読み上げ）。 */
	function fallBack(reason: string, retry = false) {
		if (serverAvailable.value) status.value = { kind: "server", reason };
		else
			status.value = browser
				? { kind: "browser", reason, retry }
				: { kind: "none", reason };
	}

	/** この端末で作るときは、今の声の準備を先に始めておく（最初の読み上げを速くする）。 */
	function warmUp() {
		if (status.value.kind === "ready")
			void client.prepareVoice(options.voice()).catch(() => undefined);
	}

	/** この端末に音声モデルを（無ければダウンロードして）読み込む。 */
	async function prepareModel() {
		status.value = {
			kind: "loading",
			loaded: 0,
			total:
				status.value.kind === "needs-download" ? status.value.totalBytes : 1,
		};
		try {
			const adapter = await client.load((loaded, total) => {
				status.value = { kind: "loading", loaded, total };
			});
			if (disposed) return;
			status.value = { kind: "ready", adapter };
			warmUp();
		} catch (cause) {
			const reason = cause instanceof Error ? cause.message : String(cause);
			fallBack(`この端末で音声モデルを読み込めませんでした: ${reason}`, true);
		}
	}

	/** どこで声を作るかを決める（画面を開いたとき・設定を変えたとき・モデルを消したとき）。 */
	async function check() {
		const [probe, server] = await Promise.all([
			client.probe(),
			serverTtsAvailable(),
		]);
		serverAvailable.value = server;
		if (options.useServer.value && server) {
			status.value = {
				kind: "server",
				reason: "設定で、サーバーで作るようにしています",
			};
			return;
		}
		if (!probe.supported) {
			fallBack(probe.reason ?? "この端末では声を作れません");
			return;
		}
		if (probe.stored) await prepareModel();
		else
			status.value = { kind: "needs-download", totalBytes: probe.totalBytes };
	}

	/** この端末に保存した音声モデルを消し、どこで声を作るかを決め直す。 */
	async function clearStoredModel() {
		await client.clearStored();
		await check();
	}

	/** ユーザー操作の中で音を鳴らせる状態にしておく（自動再生の制限）。 */
	const unlockAudio = () => player.unlock().catch(() => undefined);

	/** 声のサンプル（48 kHz モノラル）から話者条件を作る（この端末で作れればこの端末で、無ければサーバーで）。 */
	function clone(audio: Float32Array): Promise<SpeakerCondition> {
		// 送ると Worker に移るので、やり直せるよう複製を渡す。
		return status.value.kind === "ready"
			? client.clone(audio.slice())
			: serverClone(audio);
	}

	/** 特徴（キャプション）と声の番号から話者条件を作る。 */
	function design(
		caption: string,
		voiceSeed: number,
	): Promise<SpeakerCondition> {
		return status.value.kind === "ready"
			? client.design(caption, voiceSeed)
			: serverDesign(caption, voiceSeed);
	}

	return {
		status,
		serverAvailable,
		canUseIrodori,
		speaker,
		warmUp,
		prepareModel,
		check,
		clearStoredModel,
		unlockAudio,
		clone,
		design,
	};
}
