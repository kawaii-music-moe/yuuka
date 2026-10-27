<script lang="ts">
// パスワード・トークン・API キーの入力欄。
// type="password" だとブラウザやパスワードマネージャーが保存済みパスワードを候補に出すため、
// text 型にして表示だけ伏せ字（.secret-input）にし、各パスワードマネージャーの除外属性も付ける。
import type { HTMLInputAttributes } from "svelte/elements";

interface Props extends Omit<HTMLInputAttributes, "type" | "value"> {
	value: string;
}
let { value = $bindable(""), class: className = "", ...rest }: Props = $props();
</script>

<input
	type="text"
	class="secret-input {className}"
	autocomplete="off"
	autocapitalize="off"
	spellcheck="false"
	data-1p-ignore
	data-lpignore="true"
	data-bwignore
	data-form-type="other"
	{...rest}
	bind:value
/>
