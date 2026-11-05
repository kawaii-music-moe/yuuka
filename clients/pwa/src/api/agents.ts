import { ref } from "vue";
import { readStorage, removeStorage, writeStorage } from "../utils/storage";
import { request } from "./http";

/** 切り替え先のエージェント（サーバーの Bot）。 */
export type Agent = { id: string; name: string };

/**
 * システム Bot。Yuuka の使い方・設定の案内役で個人データを持たないため、PWA では扱わない
 * （切り替え候補に出さず、サーバーも `/api/client/*` で 403 にする）。
 */
export const SYSTEM_AGENT_ID = "system_default";

const STORAGE_KEY = "yuuka.pwa.agentId";

/** 前回選んだエージェント。保存が無い・読めないときは未選択にし、一覧の取得後に先頭のエージェントを選ぶ。 */
function loadStoredAgentId(): string {
	const stored = readStorage(STORAGE_KEY) ?? "";
	return stored === SYSTEM_AGENT_ID ? "" : stored;
}

/** 選択中のエージェント（未選択は空文字）。`/api/client/*` はすべてこの Bot のデータを読み書きする。 */
export const selectedAgentId = ref(loadStoredAgentId());

export function selectAgent(id: string) {
	selectedAgentId.value = id;
	if (id) writeStorage(STORAGE_KEY, id);
	else removeStorage(STORAGE_KEY);
}

/** `/api/client/*` のパスに選択中エージェントの `botId` を付ける。 */
export function withSelectedAgent(path: string): string {
	return `${path}${path.includes("?") ? "&" : "?"}botId=${encodeURIComponent(selectedAgentId.value)}`;
}

type BotsResponse = { bots: { id: string; name: string }[] };

/** `/api/bots` の一覧（自分の Bot・共有された Bot・システム Bot）から、PWA で扱うエージェントだけを残す。 */
export function toAgents(bots: BotsResponse["bots"]): Agent[] {
	return bots
		.filter((bot) => bot.id !== SYSTEM_AGENT_ID)
		.map((bot) => ({ id: bot.id, name: bot.name }));
}

/** 切り替えられるエージェント。サイドバーと設定画面で共有し、どちらで取り直しても両方に反映する。 */
export const agents = ref<Agent[]>([]);

/**
 * 切り替えられるエージェントを取り直す。選択中のエージェントが一覧に無い（未選択・削除・共有解除）場合は
 * 先頭のエージェントを選ぶ（1 つも無ければ未選択）。取得に失敗したときは直前の一覧と選択を残す
 * （一時的な失敗で選択欄が消えないように）。
 */
export async function refreshAgents(): Promise<Agent[]> {
	const { bots } = await request<BotsResponse>("/api/bots");
	const list = toAgents(bots);
	agents.value = list;
	if (!list.some((agent) => agent.id === selectedAgentId.value))
		selectAgent(list[0]?.id ?? "");
	return list;
}
