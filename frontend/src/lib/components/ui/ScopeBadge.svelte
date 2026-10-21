<script lang="ts" module>
// 設定の適用範囲バッジ（docs/design/settings-restructure.md「方針 6」）。
// 「この設定がどこまで効くか」を各ページ・各カードの見出しに出す。
export type SettingsScope = "bot" | "user_bot" | "user" | "client" | "system";

const SCOPES: Record<
	SettingsScope,
	{ label: string; title: string; icon: string }
> = {
	bot: {
		label: "このBot",
		title: "選択中のBotに効きます",
		icon: "smart_toy",
	},
	user_bot: {
		label: "あなた × このBot",
		title: "あなたがこのBotを使うときだけ効きます",
		icon: "person",
	},
	user: {
		label: "あなた（全Bot共通）",
		title: "あなたのすべてのBotに効きます",
		icon: "person",
	},
	client: {
		label: "この端末",
		title: "このブラウザ（端末）だけに保存されます",
		icon: "devices",
	},
	system: {
		label: "システム全体",
		title: "すべてのユーザーとBotに効きます",
		icon: "public",
	},
};
</script>

<script lang="ts">
	import Icon from "./Icon.svelte";

	interface Props {
		scope: SettingsScope;
		class?: string;
	}

	let { scope, class: klass = "" }: Props = $props();

	const def = $derived(SCOPES[scope]);
</script>

<span class="scope-badge {klass}" title={def.title}>
	<Icon name={def.icon} size={14} />{def.label}
</span>

<style>
	.scope-badge {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		padding: 2px 8px;
		border: 1px solid var(--border-divider);
		border-radius: 999px;
		font-size: 0.72rem;
		font-weight: 600;
		line-height: 1.6;
		white-space: nowrap;
		color: var(--text-secondary);
		vertical-align: middle;
	}
</style>
