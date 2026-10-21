// UIモック entry（/mock.html）。fetch を mockApi に差し替えた上で、
// 実物の BotDiscord（Bot設定 › Discord）をそのままマウントする。汎用モードの各セクションは
// ページ内タブ（応答する場所／応答する相手／ギルドノート）の中にある。
import { mount } from "svelte";
import { activeBot } from "$lib/stores/activeBot";
import BotDiscord from "../routes/BotDiscord.svelte";
import { installMockApi } from "./mockApi";
import "../styles.css";

installMockApi();

// mcp_assistant プリセットの非デフォルトBotを選択状態にして owner ビューを表示させる
activeBot.set({
	id: "bot_mock_ui",
	name: "モックBot",
	avatar: "",
	preset: "mcp_assistant",
});

const target = document.getElementById("app");
if (!target) throw new Error("#app mount target not found");

mount(BotDiscord, { target });
