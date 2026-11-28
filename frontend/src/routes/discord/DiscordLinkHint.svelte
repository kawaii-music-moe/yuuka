<script lang="ts">
// Discord 未連携の利用者への案内（docs/design/optional_discord.md §6.2）。
// Bot と Discord で話す・DM で通知を受けるには、アカウントの Discord 連携が要る。
import { onMount } from "svelte";
import { settingsApi } from "$lib/api/services";
import { withBasePath } from "$lib/router";

let show = $state(false);

onMount(async () => {
	try {
		const res = await settingsApi.discordLink();
		show = !res.linked;
	} catch {
		// 状態を取れなければ案内は出さない。
	}
});
</script>

{#if show}
	<div class="config-card card discord-link-hint">
		<p class="description-text">
			Discord を連携していません。Discord でこの Bot と話したり DM で通知を受けたりするには、<a
				href={withBasePath("/account/profile")}>アカウント › プロフィール</a
			>の「Discord 連携」から連携してください。Web・PWA・デスクトップでの利用には連携は要りません。
		</p>
	</div>
{/if}
