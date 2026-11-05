<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { ApiError, agentGateway, type ChatMessage } from "@/api";
import AppIcon from "@/components/AppIcon.vue";
import MarkdownContent from "@/components/MarkdownContent.vue";
import UiButton from "@/components/UiButton.vue";
import UiDialog from "@/components/UiDialog.vue";
import VoiceEngineNotice from "@/components/voice/VoiceEngineNotice.vue";
import VoiceSampleForm from "@/components/voice/VoiceSampleForm.vue";
import { describeChatError, SLOW_REPLY_HINT_MS } from "@/utils/chatReply";
import { describeCurrentVoice, describeEngine } from "@/voice/engineStatus";
import {
	DEFAULT_CAPTION,
	DESIGNED_VOICE,
	loadVoicePreferences,
	SPEEDS,
	saveVoicePreference,
} from "@/voice/preferences";
import { speechSentences } from "@/voice/replyText";
import type { ReplySpeaker, SpeechStats } from "@/voice/speaker";
import { listenOnce, speechInputAvailable } from "@/voice/speechInput";
import type { SpeakerCondition } from "@/voice/tts/pipeline";
import type { VoiceSpec } from "@/voice/tts/protocol";
import { useVoiceEngine } from "@/voice/useVoiceEngine";
import type { VoiceSample } from "@/voice/voiceSample";
import {
	deleteVoice,
	listVoices,
	loadSpeaker,
	type SavedVoice,
	saveVoice,
	updateVoice,
	usable,
} from "@/voice/voiceStore";

// チャットを音声だけで使う画面。話す → 送信 → 返答を読み上げる、を 1 往復とし、
// ハンズフリーなら読み終えたら次の発話を聞き始める。チャットの履歴はチャット画面と共通。
type Phase = "idle" | "listening" | "thinking" | "speaking";

const PREVIEW_TEXT = "こんにちは。この声で読み上げますね。";

// この端末に保存する設定（変更を確定したときに保存する）。
const preferences = loadVoicePreferences();
const caption = ref(preferences.caption);
const handsFree = ref(preferences.handsFree);
const voiceSeed = ref(preferences.voiceSeed);
const selectedVoice = ref(preferences.selectedVoice);
const speed = ref(preferences.speed);
const useServer = ref(preferences.useServer);
/** 特徴から作る声のキャプション（空なら既定の声）。 */
const designCaption = computed(() => caption.value.trim() || DEFAULT_CAPTION);

// 会話
const phase = ref<Phase>("idle");
const heard = ref("");
const reply = ref<ChatMessage>();
const typed = ref("");
const error = ref("");
const slow = ref(false);
const stats = ref<SpeechStats>();
const canListen = speechInputAvailable();

// 声
const savedVoices = ref<SavedVoice[]>([]);
/** 保存した声の話者条件（読み込めたものだけ）。 */
const speakerConditions = new Map<string, SpeakerCondition>();
const selectedSavedVoice = computed(() =>
	savedVoices.value.find((v) => v.id === selectedVoice.value),
);
const voicesError = ref("");
const cloning = ref(false);
const savingDesigned = ref(false);
const previewing = ref(false);
const sampleForm = ref<InstanceType<typeof VoiceSampleForm>>();
/** 声を作るモーダル（特徴から・サンプルから）。 */
const createOpen = ref(false);
const createMode = ref<"design" | "sample">("design");
const createError = ref("");

/**
 * 今選んでいる声。保存した声の話者条件がまだ読めていなければ、特徴から作る声にする。
 * 声を作るモーダルで特徴から作っている間は、選んでいる声によらず、その声を試す。
 */
function voiceSpec(): VoiceSpec {
	const designing = createOpen.value && createMode.value === "design";
	const saved = designing ? undefined : selectedSavedVoice.value;
	const condition = saved && speakerConditions.get(saved.id);
	// key はサーバーでの生成が話者条件を覚えておくため（同じ声なら毎回送らない）。
	if (saved && condition)
		return {
			kind: "clone",
			caption: saved.caption,
			state: condition.state,
			mask: condition.mask,
			key: `${saved.id}-${saved.createdAt}`,
		};
	return {
		kind: "design",
		caption: designCaption.value,
		voiceSeed: voiceSeed.value,
	};
}

const {
	status: engine,
	serverAvailable,
	canUseIrodori,
	speaker: currentSpeaker,
	warmUp,
	prepareModel,
	check: checkEngine,
	clearStoredModel,
	unlockAudio,
	clone: cloneVoice,
	design: designVoice,
} = useVoiceEngine({ voice: voiceSpec, speed, useServer });

/** 最後に読み上げを始めた仕組み（止めるため）。 */
let speaker: ReplySpeaker | undefined;
let listening: AbortController | undefined;
let waiting: AbortController | undefined;
let unmounted = false;

const engineText = computed(() => describeEngine(engine.value));
const phaseText = computed(
	() =>
		({
			idle: canListen
				? "マイクを押して話しかけてください"
				: "下の欄に入力して送信してください",
			listening: "聞いています…（もう一度押すと送ります）",
			thinking: slow.value
				? "考えています（時間がかかっています）…"
				: "考えています…",
			speaking: "話しています（押すと止めて、次を聞きます）",
		})[phase.value],
);
const micIcon = computed(() =>
	phase.value === "speaking"
		? "stop"
		: phase.value === "thinking"
			? "more_horiz"
			: "mic",
);
/** マイクの下に出す「今の声」。 */
const currentVoiceLabel = computed(() =>
	describeCurrentVoice(
		engine.value,
		selectedSavedVoice.value?.name ??
			`特徴から作る声（声 ${voiceSeed.value + 1}）`,
		serverAvailable.value,
	),
);
const canPreview = computed(
	() => canUseIrodori.value && !previewing.value && phase.value === "idle",
);

const describeError = (cause: unknown) =>
	describeChatError(cause, {
		timeout:
			"エージェントの応答を待ちきれませんでした。チャット画面で履歴を確認してください。",
		detail: true,
	});

/** 聞き取った（入力した）文を送り、返答を読み上げる。 */
async function exchange(text: string) {
	heard.value = text;
	reply.value = undefined;
	phase.value = "thinking";
	slow.value = false;
	waiting = new AbortController();
	const message = await agentGateway.sendChatMessage(text, [], {
		signal: waiting.signal,
		onTick: ({ elapsedMs }) => {
			slow.value = elapsedMs >= SLOW_REPLY_HINT_MS;
		},
	});
	if (unmounted) return;
	reply.value = message;
	speaker = currentSpeaker();
	if (!speaker) {
		phase.value = "idle";
		return;
	}
	phase.value = "speaking";
	const result = await speaker.speak(speechSentences(message.content));
	if (result) stats.value = result;
}

/** 1 往復（聞く → 送る → 読み上げる）。ハンズフリーなら続けて次を聞く。 */
async function turn() {
	error.value = "";
	try {
		for (;;) {
			phase.value = "listening";
			heard.value = "";
			listening = new AbortController();
			const text = await listenOnce({
				signal: listening.signal,
				onInterim: (t) => {
					heard.value = t;
				},
			});
			listening = undefined;
			if (unmounted || phase.value !== "listening") return;
			if (!text) {
				phase.value = "idle";
				return;
			}
			await exchange(text);
			// 読み上げ中に止められた（別の往復が始まった）ら、この往復はここで終える。
			if (unmounted || (phase.value as Phase) !== "speaking") return;
			phase.value = "idle";
			if (!handsFree.value) return;
		}
	} catch (cause) {
		if (unmounted || waiting?.signal.aborted) return;
		error.value = describeError(cause);
		phase.value = "idle";
	}
}

async function onMic() {
	await unlockAudio();
	switch (phase.value) {
		case "listening":
			listening?.abort();
			return;
		case "thinking":
			return;
		case "speaking":
			speaker?.stop();
			phase.value = "idle";
			if (canListen) void turn();
			return;
		case "idle":
			if (canListen) void turn();
	}
}

async function sendTyped() {
	const text = typed.value.trim();
	if (!text || phase.value === "thinking" || phase.value === "listening")
		return;
	await unlockAudio();
	speaker?.stop();
	typed.value = "";
	error.value = "";
	try {
		await exchange(text);
		if (!unmounted && phase.value === "speaking") phase.value = "idle";
	} catch (cause) {
		if (unmounted) return;
		error.value = describeError(cause);
		phase.value = "idle";
	}
}

function saveCaption() {
	saveVoicePreference("caption", designCaption.value);
	warmUp();
}
function saveSpeed() {
	saveVoicePreference("speed", speed.value);
}
function saveHandsFree() {
	saveVoicePreference("handsFree", handsFree.value);
}
async function saveUseServer() {
	saveVoicePreference("useServer", useServer.value);
	speaker?.stop();
	await checkEngine();
}
async function clearModel() {
	speaker?.stop();
	await clearStoredModel();
}

/** 保存した声の話者条件を読み込む（端末のキャッシュか、無ければサーバーから）。 */
async function ensureSpeaker(voice: SavedVoice): Promise<boolean> {
	if (speakerConditions.has(voice.id)) return true;
	try {
		speakerConditions.set(voice.id, await loadSpeaker(voice));
		return true;
	} catch (cause) {
		voicesError.value =
			cause instanceof Error
				? `声「${voice.name}」を読み込めませんでした: ${cause.message}`
				: `声「${voice.name}」を読み込めませんでした。`;
		return false;
	}
}

async function selectVoice() {
	saveVoicePreference("selectedVoice", selectedVoice.value);
	const saved = selectedSavedVoice.value;
	if (saved && !(await ensureSpeaker(saved))) return;
	warmUp();
}

async function loadSavedVoices() {
	voicesError.value = "";
	try {
		savedVoices.value = await listVoices();
	} catch {
		voicesError.value = "保存した声の一覧を取得できませんでした。";
		return;
	}
	const selected = selectedSavedVoice.value;
	if (selected && usable(selected)) await ensureSpeaker(selected);
	else if (selectedVoice.value !== DESIGNED_VOICE) {
		selectedVoice.value = DESIGNED_VOICE;
		saveVoicePreference("selectedVoice", DESIGNED_VOICE);
	}
}

/** 保存した声の説明（一覧に出す）。 */
function voiceSummary(voice: SavedVoice): string {
	if (!usable(voice)) return "今の音声モデルでは使えません";
	return voice.source === "sample"
		? `${voice.seconds} 秒のサンプルから`
		: "特徴から作った声";
}

/** 作った声を一覧に加えて選び、試しに読む。 */
async function adopt(saved: SavedVoice, condition: SpeakerCondition) {
	closeCreate();
	speakerConditions.set(saved.id, condition);
	savedVoices.value = [...savedVoices.value, saved];
	selectedVoice.value = saved.id;
	await selectVoice();
	await preview();
}

/** 声の保存・変更に失敗したときの案内文。 */
function failedMessage(cause: unknown, what: string): string {
	if (cause instanceof ApiError && cause.serverMessage)
		return `${what}: ${cause.serverMessage}`;
	return cause instanceof Error ? `${what}: ${cause.message}` : `${what}。`;
}

/** 声のサンプルから声を作ってサーバーに保存し、その声に切り替えて試しに読む。 */
async function addVoice({
	name,
	caption: style,
	sample,
}: {
	name: string;
	caption: string;
	sample: VoiceSample;
}) {
	cloning.value = true;
	createError.value = "";
	try {
		const condition = await cloneVoice(sample.audio);
		const saved = await saveVoice(
			{
				name,
				caption: style,
				source: "sample",
				seconds: Math.round(sample.seconds * 10) / 10,
			},
			condition,
		);
		sampleForm.value?.reset();
		await adopt(saved, condition);
	} catch (cause) {
		createError.value = failedMessage(cause, "声を作れませんでした");
	} finally {
		cloning.value = false;
	}
}

/** 特徴から作った今の声を保存する（どの端末でも同じ声で読める）。 */
async function saveDesigned() {
	savingDesigned.value = true;
	createError.value = "";
	try {
		const style = designCaption.value;
		const condition = await designVoice(style, voiceSeed.value);
		const name = `${[...style].slice(0, 16).join("")}（声 ${voiceSeed.value + 1}）`;
		await adopt(
			await saveVoice(
				{ name, caption: style, source: "design", seconds: 0 },
				condition,
			),
			condition,
		);
	} catch (cause) {
		createError.value = failedMessage(cause, "声を保存できませんでした");
	} finally {
		savingDesigned.value = false;
	}
}

function replaceVoice(updated: SavedVoice) {
	savedVoices.value = savedVoices.value.map((v) =>
		v.id === updated.id ? updated : v,
	);
}

async function renameVoice(voice: SavedVoice) {
	const name = voice.name.trim();
	if (!name) return loadSavedVoices();
	try {
		replaceVoice(await updateVoice(voice.id, { name }));
	} catch (cause) {
		voicesError.value = failedMessage(cause, "名前を変えられませんでした");
	}
}

async function saveVoiceStyle(voice: SavedVoice) {
	try {
		replaceVoice(
			await updateVoice(voice.id, { caption: voice.caption.trim() }),
		);
	} catch (cause) {
		voicesError.value = failedMessage(
			cause,
			"話し方の指定を保存できませんでした",
		);
	}
}

async function removeVoice(voice: SavedVoice) {
	if (
		!window.confirm(
			`「${voice.name}」を削除しますか？（ほかの端末からも消えます）`,
		)
	)
		return;
	try {
		await deleteVoice(voice);
	} catch (cause) {
		voicesError.value = failedMessage(cause, "声を削除できませんでした");
		return;
	}
	savedVoices.value = savedVoices.value.filter((v) => v.id !== voice.id);
	speakerConditions.delete(voice.id);
	if (selectedVoice.value === voice.id) {
		selectedVoice.value = DESIGNED_VOICE;
		await selectVoice();
	}
}

/** 今の声で短い文を読む。 */
async function preview() {
	if (phase.value !== "idle" || previewing.value) return;
	await unlockAudio();
	previewing.value = true;
	// モーダルの中で試したときは、エラーもモーダルに出す。
	const report = createOpen.value ? createError : error;
	report.value = "";
	try {
		speaker = currentSpeaker();
		await speaker?.speak([PREVIEW_TEXT]);
	} catch (cause) {
		report.value = describeError(cause);
	} finally {
		previewing.value = false;
	}
}
/** 同じキャプションで別の声にして、試しに読む。 */
async function nextVoice() {
	voiceSeed.value += 1;
	saveVoicePreference("voiceSeed", voiceSeed.value);
	await preview();
}

function openCreate() {
	createMode.value = "design";
	createError.value = "";
	createOpen.value = true;
}
function switchCreateMode(mode: "design" | "sample") {
	createMode.value = mode;
	createError.value = "";
}
/** モーダルを閉じる（Esc・背景・閉じるボタン、声を保存したとき）。試している声は止め、選んでいる声に戻す。 */
function closeCreate() {
	if (!createOpen.value) return;
	if (previewing.value) speaker?.stop();
	createOpen.value = false;
	warmUp();
}

onMounted(() => {
	void loadSavedVoices();
	checkEngine().catch((cause) => {
		engine.value = { kind: "none", reason: describeError(cause) };
	});
});
onBeforeUnmount(() => {
	unmounted = true;
	listening?.abort();
	waiting?.abort();
	speaker?.stop();
});
</script>

<template>
  <section class="voice-page">
    <div class="voice-intro"><RouterLink to="/chat" class="text-link">チャットの履歴</RouterLink></div>

    <!-- 読み上げの準備（Irodori-TTS の声で読めないときだけ目立たせる） -->
    <VoiceEngineNotice v-if="!canUseIrodori" :status="engine" @prepare="prepareModel" />

    <!-- 会話 -->
    <div class="stage">
      <button class="mic" :class="phase" type="button" :disabled="phase === 'thinking' || (!canListen && phase !== 'speaking')" :aria-label="phaseText" @click="onMic">
        <AppIcon :name="micIcon" />
      </button>
      <p class="phase-text" role="status">{{ phaseText }}</p>
      <p class="voice-line muted"><AppIcon name="record_voice_over" />{{ currentVoiceLabel }}</p>
      <label class="toggle"><input v-model="handsFree" type="checkbox" @change="saveHandsFree" /><span>ハンズフリー</span></label>
      <p v-if="error" class="error">{{ error }}</p>
    </div>

    <form v-if="!canListen" class="typed" @submit.prevent="sendTyped">
      <p class="muted">このブラウザは音声入力に対応していません。文字で送ると、返答を読み上げます。</p>
      <div class="typed-row">
        <input v-model="typed" type="text" placeholder="メッセージ" />
        <UiButton type="submit" icon="send" :disabled="!typed.trim() || phase === 'thinking'">送信</UiButton>
      </div>
    </form>

    <div v-if="heard || reply" class="exchange">
      <div v-if="heard" class="bubble user"><span class="who">あなた</span><p>{{ heard }}</p></div>
      <div v-if="reply" class="bubble agent"><span class="who">エージェント</span><MarkdownContent :source="reply.content" /></div>
    </div>

    <!-- 声 -->
    <section v-if="engine.kind !== 'browser' && engine.kind !== 'none'" class="panel surface">
      <div class="panel-head">
        <h2 class="section-title">声</h2>
        <UiButton variant="secondary" icon="add" @click="openCreate">声を作る</UiButton>
      </div>
      <div class="voice" :class="{ active: selectedVoice === DESIGNED_VOICE }">
        <label class="voice-head">
          <input v-model="selectedVoice" type="radio" :value="DESIGNED_VOICE" @change="selectVoice" />
          <span class="voice-name">特徴から作る声</span>
          <span class="muted">{{ designCaption }}・声 {{ voiceSeed + 1 }}</span>
        </label>
        <div v-if="selectedVoice === DESIGNED_VOICE" class="row">
          <UiButton variant="secondary" icon="play_arrow" :disabled="!canPreview" @click="preview">試しに聞く</UiButton>
        </div>
      </div>

      <p v-if="savedVoices.length > 0" class="group-title">保存した声<span class="muted">（あなただけが、ほかの端末からも使えます）</span></p>
      <div v-for="v in savedVoices" :key="v.id" class="voice" :class="{ active: selectedVoice === v.id, unusable: !usable(v) }">
        <label class="voice-head">
          <input v-model="selectedVoice" type="radio" :value="v.id" :disabled="!usable(v)" @change="selectVoice" />
          <span class="voice-name">{{ v.name }}</span>
          <span class="muted">{{ voiceSummary(v) }}</span>
        </label>
        <template v-if="selectedVoice === v.id">
          <label class="field"><span>名前</span><input v-model="v.name" type="text" maxlength="40" @change="renameVoice(v)" /></label>
          <label class="field"><span>話し方の指定（任意・例: 明るく、少し早口で）</span><input v-model="v.caption" type="text" maxlength="200" @change="saveVoiceStyle(v)" /></label>
          <div class="row">
            <UiButton variant="secondary" icon="play_arrow" :disabled="!canPreview" @click="preview">試しに聞く</UiButton>
            <UiButton variant="text" icon="delete" @click="removeVoice(v)">削除</UiButton>
          </div>
        </template>
      </div>
      <p v-if="voicesError" class="error">{{ voicesError }}</p>
    </section>

    <!-- 声を作る（閉じると中身を捨てる。録音中なら止まる） -->
    <UiDialog :open="createOpen" :width="520" aria-labelledby="create-title" @close="closeCreate">
      <div v-if="createOpen" class="create-body">
        <h2 id="create-title">声を作る</h2>
        <div class="tabs" role="tablist">
          <button type="button" role="tab" :aria-selected="createMode === 'design'" :class="{ active: createMode === 'design' }" @click="switchCreateMode('design')">特徴から作る</button>
          <button type="button" role="tab" :aria-selected="createMode === 'sample'" :class="{ active: createMode === 'sample' }" @click="switchCreateMode('sample')">声のサンプルから作る</button>
        </div>
        <div v-if="createMode === 'design'" class="design-form" role="tabpanel">
          <label class="field"><span>どんな声にするか</span><input v-model="caption" type="text" maxlength="200" autofocus @change="saveCaption" /></label>
          <div class="row">
            <UiButton variant="secondary" icon="play_arrow" :disabled="!canPreview" @click="preview">試しに聞く</UiButton>
            <UiButton variant="secondary" icon="refresh" :disabled="!canPreview" @click="nextVoice">別の声にする</UiButton>
            <span class="muted seed">声 {{ voiceSeed + 1 }}</span>
          </div>
          <div class="row">
            <UiButton icon="bookmark_add" :disabled="!canUseIrodori || savingDesigned" @click="saveDesigned">{{ savingDesigned ? '保存しています…' : 'この声を保存' }}</UiButton>
          </div>
        </div>
        <VoiceSampleForm v-else ref="sampleForm" role="tabpanel" :disabled="!canUseIrodori" :busy="cloning" @create="addVoice" />
        <p v-if="!canUseIrodori" class="muted">音声モデルの準備ができると、声を作れます。</p>
        <p v-if="createError" class="error">{{ createError }}</p>
        <div class="create-actions"><UiButton variant="secondary" @click="closeCreate">閉じる</UiButton></div>
      </div>
    </UiDialog>

    <!-- 読み上げの設定 -->
    <section class="panel surface">
      <h2 class="section-title">読み上げの設定</h2>
      <label class="field">
        <span>話す速さ</span>
        <select v-model.number="speed" @change="saveSpeed">
          <option v-for="s in SPEEDS" :key="s.value" :value="s.value">{{ s.label }}</option>
        </select>
      </label>
      <details class="more">
        <summary><AppIcon name="tune" />詳しい情報</summary>
        <div class="details-body">
          <p v-if="engineText" class="muted">{{ engineText }}</p>
          <label v-if="serverAvailable" class="toggle"><input v-model="useServer" type="checkbox" @change="saveUseServer" /><span>この端末の GPU を使わず、サーバーで声を作る</span></label>
          <p v-if="stats" class="muted">前回の読み上げ: 最初の音まで {{ (stats.firstAudioMs / 1000).toFixed(1) }} 秒・途切れ {{ (stats.stallMs / 1000).toFixed(1) }} 秒・音声 {{ stats.audioSec.toFixed(1) }} 秒</p>
          <div v-if="engine.kind === 'ready'" class="row"><UiButton variant="text" icon="delete" @click="clearModel">この端末に保存した音声モデルを削除</UiButton></div>
        </div>
      </details>
    </section>
  </section>
</template>

<style scoped>
.voice-page { max-width: 720px; margin: 0 auto; display: flex; flex-direction: column; gap: 16px; }
.voice-intro { display: flex; justify-content: flex-end; gap: 12px; align-items: end; border-bottom: 1px solid var(--line); padding-bottom: 15px; }
.text-link { font-size: 13px; color: var(--primary); white-space: nowrap; }
.stage { display: flex; flex-direction: column; align-items: center; gap: 8px; padding: 24px 0 8px; text-align: center; }
.mic { width: 112px; height: 112px; border-radius: 50%; border: 2px solid var(--primary-border); background: var(--primary-soft); color: var(--primary); display: grid; place-items: center; transition: transform .15s ease,background .15s ease; }
.mic .app-icon { font-size: 48px; }
.mic:hover:not(:disabled) { background: rgba(187,134,252,.2); }
.mic:disabled { opacity: .6; cursor: default; }
.mic.listening { background: var(--primary); color: #000; animation: pulse 1.4s ease-in-out infinite; }
.mic.speaking { border-color: var(--primary); }
@keyframes pulse {
  0%, 100% { box-shadow: 0 0 0 0 rgba(187,134,252,.45); }
  50% { box-shadow: 0 0 0 16px rgba(187,134,252,0); }
}
@media (prefers-reduced-motion: reduce) {
  .mic.listening { animation: none; }
}
.phase-text { margin: 6px 0 0; font-size: 16px; font-weight: 700; }
.voice-line { display: flex; align-items: center; gap: 6px; margin: 0; }
.voice-line .app-icon { font-size: 17px; }
.toggle { display: flex; gap: 8px; align-items: center; font-size: 13px; margin-top: 4px; cursor: pointer; }
.stage .error { margin: 4px 0 0; }
.typed-row { display: flex; gap: 8px; }
.typed-row input { flex: 1; min-width: 0; padding: 9px 10px; border: 1px solid var(--field); border-radius: 4px; }
.exchange { display: flex; flex-direction: column; gap: 10px; }
.bubble { padding: 10px 14px; border-radius: 4px; overflow-wrap: anywhere; }
.bubble p { margin: 0; }
.bubble .who { display: block; font-size: 11px; font-weight: 700; color: var(--text-medium); margin-bottom: 4px; }
.bubble.user { align-self: flex-end; max-width: 85%; background: var(--primary-soft); border-left: 3px solid var(--primary); }
.bubble.agent { border: 1px solid var(--line); border-left: 2px solid var(--line); }
.panel { padding: 14px 16px; display: flex; flex-direction: column; gap: 10px; }
.panel .section-title { margin: 0 0 2px; font-size: 15px; }
.panel-head { display: flex; justify-content: space-between; align-items: center; gap: 8px; }
.panel-head .section-title { margin: 0; }
.group-title { margin: 8px 0 0; font-size: 13px; font-weight: 600; }
.group-title .muted { font-weight: 400; margin-left: 4px; }
.voice { display: flex; flex-direction: column; gap: 10px; padding: 10px 12px; border: 1px solid var(--line); border-radius: 4px; }
.voice.active { border-color: var(--primary-border); background: var(--primary-soft); }
.voice.unusable { opacity: .6; }
.voice-head { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; cursor: pointer; }
.voice-name { font-weight: 600; font-size: 14px; }
.voice-head .muted { font-size: 12px; }
.row { display: flex; gap: 8px; flex-wrap: wrap; align-items: center; }
.field { display: flex; flex-direction: column; gap: 5px; font-size: 12px; color: var(--text-medium); }
.field input { padding: 9px 10px; border: 1px solid var(--field); border-radius: 4px; font-size: 14px; }
.field select { padding: 9px 10px; border: 1px solid var(--field); border-radius: 4px; font-size: 14px; max-width: 220px; }
.more { border-top: 1px solid var(--line); padding-top: 10px; }
.more>summary { display: flex; align-items: center; gap: 6px; cursor: pointer; font-size: 13px; font-weight: 600; color: var(--primary); list-style: none; }
.more>summary::-webkit-details-marker { display: none; }
.more[open]>summary { margin-bottom: 10px; }
.details-body { display: flex; flex-direction: column; gap: 8px; }
.details-body .muted { margin: 0; }
.create-body { padding: 20px; display: flex; flex-direction: column; gap: 14px; }
.create-body h2 { margin: 0; font-size: 16px; }
.create-body>.muted, .create-body>.error { margin: 0; font-size: 13px; }
.tabs { display: flex; border-bottom: 1px solid var(--line); }
.tabs button { flex: 1; padding: 8px 10px; border: 0; border-bottom: 2px solid transparent; margin-bottom: -1px; background: transparent; color: var(--text-medium); font: inherit; font-size: 13px; font-weight: 600; cursor: pointer; }
.tabs button.active { color: var(--primary); border-bottom-color: var(--primary); }
.tabs button:focus { outline: 0; }
.tabs button:focus-visible { outline: 2px solid var(--primary-border); outline-offset: -2px; }
.design-form { display: flex; flex-direction: column; gap: 10px; }
.seed { font-size: 12px; }
.create-actions { display: flex; justify-content: flex-end; }
@media (max-width: 640px) {
  .stage { padding: 16px 0 4px; }
  .mic { width: 96px; height: 96px; }
}
</style>
