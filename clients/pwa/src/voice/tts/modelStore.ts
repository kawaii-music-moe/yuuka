/**
 * 音声モデル（約 1.7 GB）の取得と端末への保存（OPFS）。
 *
 * Cache Storage は 700 MB 級の 1 ファイルを `put` できない環境があった（検証時に Iris Xe の Chrome で
 * `Unexpected internal error`）。OPFS にストリームで書き込み、2 回目以降はそこから読む。
 * 保存先のフォルダ名はモデルのバージョンで、別バージョンのフォルダは読み込み時に消す。
 */

/** サーバーの `/tts-models/<version>/` に置くモデル一式のバージョン。モデルを差し替えたら上げる。 */
export const MODEL_VERSION = "irodori-v41-mf-fp16-3";
export const MODEL_BASE = `/tts-models/${MODEL_VERSION}/`;
const STORE_PREFIX = "tts-models-";

/** `files` は最初にまとめて取得するもの、`optional` は使うときに取得するもの（声のサンプル用エンコーダ）。 */
export type ModelManifest = {
	version: string;
	files: Record<string, number>;
	optional?: Record<string, number>;
};
export type DownloadProgress = { loaded: number; total: number };

async function storeDir(
	create: boolean,
): Promise<FileSystemDirectoryHandle | null> {
	try {
		const root = await navigator.storage.getDirectory();
		return await root.getDirectoryHandle(`${STORE_PREFIX}${MODEL_VERSION}`, {
			create,
		});
	} catch {
		return null;
	}
}

export async function fetchManifest(): Promise<ModelManifest> {
	const res = await fetch(`${MODEL_BASE}manifest.json`, { cache: "no-cache" });
	if (!res.ok)
		throw new Error(`音声モデルの一覧を取得できません（${res.status}）`);
	return res.json();
}

/** 保存済みのファイルがすべて揃っているか（揃っていれば、ダウンロードの確認なしに読み込んでよい）。 */
export async function isStored(manifest: ModelManifest): Promise<boolean> {
	const dir = await storeDir(false);
	if (!dir) return false;
	for (const [name, size] of Object.entries(manifest.files)) {
		try {
			if ((await (await dir.getFileHandle(name)).getFile()).size !== size)
				return false;
		} catch {
			return false;
		}
	}
	return true;
}

/** 別バージョンの保存フォルダを消す。 */
export async function removeStaleVersions(): Promise<void> {
	try {
		const root = await navigator.storage.getDirectory();
		for await (const name of (
			root as unknown as { keys(): AsyncIterable<string> }
		).keys()) {
			if (
				name.startsWith(STORE_PREFIX) &&
				name !== `${STORE_PREFIX}${MODEL_VERSION}`
			)
				await root.removeEntry(name, { recursive: true });
		}
	} catch {
		// 保存先が使えない環境では何もしない。
	}
}

export async function clearStoredModels(): Promise<void> {
	const root = await navigator.storage.getDirectory();
	await root
		.removeEntry(`${STORE_PREFIX}${MODEL_VERSION}`, { recursive: true })
		.catch(() => undefined);
}

/**
 * モデルのファイルを返す。保存済みならそこから、無ければ取得しながら保存する。
 * 保存できない環境（OPFS 非対応・容量不足）では、保存せずにメモリへ読み込む。
 */
export async function loadModelFile(
	manifest: ModelManifest,
	name: string,
	onProgress: (bytes: number) => void,
): Promise<Uint8Array> {
	const size = manifest.files[name] ?? manifest.optional?.[name];
	if (size === undefined) throw new Error(`音声モデルに ${name} がありません`);
	const dir = await storeDir(true);
	if (dir) {
		try {
			const handle = await dir.getFileHandle(name, { create: true });
			const stored = await handle.getFile();
			if (stored.size === size) {
				onProgress(size);
				return new Uint8Array(await stored.arrayBuffer());
			}
			const res = await fetch(`${MODEL_BASE}${name}`);
			if (!res.ok || !res.body)
				throw new Error(`音声モデルを取得できません（${name}: ${res.status}）`);
			const writable = await handle.createWritable();
			const reader = res.body.getReader();
			try {
				for (;;) {
					const { value, done } = await reader.read();
					if (done) break;
					await writable.write(value);
					onProgress(value.byteLength);
				}
				await writable.close();
			} catch (error) {
				await writable.abort().catch(() => undefined);
				throw error;
			}
			const file = await handle.getFile();
			if (file.size !== size)
				throw new Error(`音声モデルの取得が途中で切れました（${name}）`);
			return new Uint8Array(await file.arrayBuffer());
		} catch (error) {
			// 取得そのものの失敗はそのまま投げる。保存先の失敗（容量不足など）なら保存せずに読む。
			if (error instanceof Error && error.message.startsWith("音声モデル"))
				throw error;
		}
	}
	const res = await fetch(`${MODEL_BASE}${name}`);
	if (!res.ok)
		throw new Error(`音声モデルを取得できません（${name}: ${res.status}）`);
	const bytes = new Uint8Array(await res.arrayBuffer());
	onProgress(bytes.byteLength);
	return bytes;
}
