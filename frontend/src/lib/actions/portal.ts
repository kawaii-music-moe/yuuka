// use:portal — 要素を document.body 直下へ移す（親の overflow / transform / z-index の影響を受けない
// ようにするため。モーダルやドロップダウンの一覧で使う）。破棄時に取り除く。
export function portal(node: HTMLElement) {
	document.body.appendChild(node);
	return {
		destroy() {
			node.remove();
		},
	};
}
