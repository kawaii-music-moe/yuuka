// 音声画面の設定。この端末（ブラウザ）だけに保存する（保存した声はサーバー・voiceStore.ts）。
import { readStorage, writeStorage } from "../utils/storage";

/** 特徴（キャプション）から作る声を選んでいるときの値。それ以外は保存した声の id。 */
export const DESIGNED_VOICE = "designed";
export const DEFAULT_CAPTION = "落ち着いた、近い距離感の若い女性話者";
/** 話す速さ（上流の duration_scale。大きいほどゆっくり）。 */
export const SPEEDS = [
	{ value: 0.85, label: "速い" },
	{ value: 0.93, label: "やや速い" },
	{ value: 1, label: "ふつう" },
	{ value: 1.1, label: "ややゆっくり" },
	{ value: 1.2, label: "ゆっくり" },
];

export type VoicePreferences = {
	/** 特徴から作る声の特徴（キャプション）。 */
	caption: string;
	/** 声の番号。キャプションが同じでも番号ごとに別の声になる（「別の声にする」で進める）。 */
	voiceSeed: number;
	/** 読み終えたら次の発話を聞き始める。 */
	handsFree: boolean;
	/** `DESIGNED_VOICE` か保存した声の id。 */
	selectedVoice: string;
	speed: number;
	/** この端末の GPU を使わず、サーバーで作る（GPU が弱い・電池を減らしたくない端末向け）。 */
	useServer: boolean;
};

const KEYS: Record<keyof VoicePreferences, string> = {
	caption: "agent-desk:voice-caption",
	voiceSeed: "agent-desk:voice-seed",
	handsFree: "agent-desk:voice-hands-free",
	selectedVoice: "agent-desk:voice-selected",
	speed: "agent-desk:voice-speed",
	useServer: "agent-desk:voice-use-server",
};

/** 保存した設定（無い・壊れている項目は既定値）。 */
export function loadVoicePreferences(): VoicePreferences {
	return {
		caption: readStorage(KEYS.caption) ?? DEFAULT_CAPTION,
		voiceSeed: Number(readStorage(KEYS.voiceSeed) ?? 0) || 0,
		handsFree: readStorage(KEYS.handsFree) === "1",
		selectedVoice: readStorage(KEYS.selectedVoice) ?? DESIGNED_VOICE,
		speed: Number(readStorage(KEYS.speed) ?? 1) || 1,
		useServer: readStorage(KEYS.useServer) === "1",
	};
}

export function saveVoicePreference<K extends keyof VoicePreferences>(
	key: K,
	value: VoicePreferences[K],
): void {
	writeStorage(
		KEYS[key],
		typeof value === "boolean" ? (value ? "1" : "0") : String(value),
	);
}
