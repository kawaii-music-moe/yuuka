<script lang="ts">
// 進捗バー。既存 .task-progress-bar / .task-progress-fill / .task-progress-text 準拠。
// 旧 buildProgressBar の置換。
interface Props {
	/** 進捗率 0..100 */
	percent: number;
	/** 右側にパーセント文字列を表示するか */
	showText?: boolean;
	/** バー塗り色（未指定なら CSS 既定 var(--color-primary)） */
	color?: string;
	class?: string;
}

let { percent, showText = false, color, class: klass = "" }: Props = $props();

const clamped = $derived(Math.max(0, Math.min(100, percent)));
const fillStyle = $derived(
	`width:${clamped}%` + (color ? `;background-color:${color}` : ""),
);
</script>

<div
	class="task-progress-bar {klass}"
	role="progressbar"
	aria-valuenow={Math.round(clamped)}
	aria-valuemin="0"
	aria-valuemax="100"
>
	<div class="task-progress-fill" style={fillStyle}></div>
</div>
{#if showText}
	<span class="task-progress-text">{Math.round(clamped)}%</span>
{/if}
