<script lang="ts">
	import type { Snippet } from "svelte";
	import Icon from "./Icon.svelte";

	// 共通モーダル。既存 .modal / .modal-content / .modal-header / .modal-footer / .active に準拠。
	// bindable open で開閉。backdrop クリック / ESC で閉じる。
	// §11.4 XSS 原則: title は文字列補間のみ（{@html} 禁止）。
	//
	// document.body へポータルする: transform/backdrop-filter を持つ祖先（.tab-view の
	// フェードイン残留 transform・overlay カードの scale 等）があると position:fixed の
	// 包含ブロックが祖先になり、モーダルが画面中央でなくカード内に極小表示されるため。
	interface Props {
		/** 開閉状態（双方向バインド） */
		open?: boolean;
		/** ヘッダタイトル（文字列。自動エスケープされる） */
		title?: string;
		/** 幅広モーダル（.wide-modal） */
		wide?: boolean;
		/** backdrop クリックで閉じるか */
		closeOnBackdrop?: boolean;
		/** ESC キーで閉じるか */
		closeOnEsc?: boolean;
		/** 閉じたときのコールバック */
		onclose?: () => void;
		class?: string;
		/** 本文 */
		children?: Snippet;
		/** フッタ（省略時は .modal-footer を描画しない） */
		footer?: Snippet;
	}

	let {
		open = $bindable(false),
		title,
		wide = false,
		closeOnBackdrop = true,
		closeOnEsc = true,
		onclose,
		class: klass = "",
		children,
		footer,
	}: Props = $props();

	/** マウント位置に依らず viewport 基準で重ねるため、ノードを body 直下へ移す。 */
	function portal(node: HTMLElement) {
		document.body.appendChild(node);
		return {
			destroy() {
				node.parentNode?.removeChild(node);
			},
		};
	}

	function close() {
		if (!open) return;
		open = false;
		onclose?.();
	}

	function onBackdrop(e: MouseEvent) {
		// backdrop（.modal 自身）のクリックのみ。content 内は無視。
		if (closeOnBackdrop && e.target === e.currentTarget) close();
	}

	function onKeydown(e: KeyboardEvent) {
		if (closeOnEsc && e.key === "Escape") {
			e.stopPropagation();
			close();
		}
	}
</script>

<svelte:window onkeydown={open ? onKeydown : undefined} />

<div
	class="modal {klass}"
	class:active={open}
	role="presentation"
	onclick={onBackdrop}
	use:portal
>
	<div
		class="modal-content"
		class:wide-modal={wide}
		role="dialog"
		aria-modal="true"
		aria-label={title}
	>
		{#if title}
			<div class="modal-header">
				<h3>{title}</h3>
				<button
					type="button"
					class="btn-close"
					aria-label="閉じる"
					onclick={close}
				>
					<Icon name="close" />
				</button>
			</div>
		{/if}

		{@render children?.()}

		{#if footer}
			<div class="modal-footer">
				{@render footer()}
			</div>
		{/if}
	</div>
</div>
