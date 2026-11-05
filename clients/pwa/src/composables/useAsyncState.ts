import { ref } from "vue";

/**
 * 画面の読み込み状態（読み込み中か・失敗したときの案内文）。画面を開いてすぐ読み込むので、
 * 既定では読み込み中から始める（読み込む前に「0 件です」のような表示を一瞬出さない）。
 *
 * `run` を続けて呼んだときは最後の呼び出しだけを反映する（期間や月を素早く切り替えたとき、
 * 古い応答で新しい表示を上書きしない）。
 */
export function useAsyncState(initiallyLoading = true) {
	const loading = ref(initiallyLoading);
	const error = ref("");
	let latest = 0;

	/** `task` の結果を返す。失敗したとき・後から別の `run` が始まったときは undefined。 */
	async function run<T>(
		task: () => Promise<T>,
		message: string,
	): Promise<T | undefined> {
		const current = ++latest;
		loading.value = true;
		error.value = "";
		try {
			const result = await task();
			return current === latest ? result : undefined;
		} catch {
			if (current === latest) error.value = message;
			return undefined;
		} finally {
			if (current === latest) loading.value = false;
		}
	}

	return { loading, error, run };
}
