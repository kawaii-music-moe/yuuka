/**
 * 一つの取得先の非同期処理を管理する。後から始めた run だけが成功・失敗・loading を反映する。
 * Svelte の $state へ代入するコールバックを渡すことで、フレームワークに依存せず使える。
 */
export function asyncState(onLoading?: (loading: boolean) => void) {
	let latest = 0;

	/** 古い結果を反映しなかったときだけ false を返す。 */
	async function run<T>(
		task: () => Promise<T>,
		apply: (result: T) => void,
		onError: (error: unknown) => void,
	): Promise<boolean> {
		const current = ++latest;
		onLoading?.(true);
		try {
			const result = await task();
			if (current === latest) apply(result);
		} catch (error) {
			if (current === latest) onError(error);
		} finally {
			if (current === latest) onLoading?.(false);
		}
		return current === latest;
	}

	/** Bot・期間の切替やコンポーネント破棄時、実行中の結果を無効にする。 */
	function cancel() {
		latest++;
		onLoading?.(false);
	}

	return { run, cancel };
}
