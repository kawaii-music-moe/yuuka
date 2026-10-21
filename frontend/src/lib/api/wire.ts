/**
 * 生成型（ts-rs）と fetch().json() の値表現の差だけを埋める変換。
 *
 * ts-rs は Rust の i64 を既定で bigint と出力するが、serde_json は JSON の数値として
 * 送り、fetch().json() は number に読む。JsonWire<T> はこの表現差だけを再帰的に
 * number へ写し、フィールド名・nullability・optional・ネストは生成型のまま保つ。
 * Number.MAX_SAFE_INTEGER を超える ID を扱うなら、先にサーバの文字列契約が必要。
 *
 * PWA（client/pwa）は別の wire 契約（string ID・camelCase・completed: boolean・
 * Envelope なしの素の JSON）を持つ。ドメイン値から PWA 用 view への変換はサーバ
 * （yuuka-client-api）で明示的に行い、PWA 側は client/pwa/src/api/contracts.ts と
 * httpAdapter.contract.test.ts で契約を固定する。管理画面の生成型を PWA から直接
 * import しない。
 */
export type JsonWire<T> = T extends bigint
	? number
	: T extends readonly (infer Item)[]
		? JsonWire<Item>[]
		: T extends object
			? { [Key in keyof T]: JsonWire<T[Key]> }
			: T;
