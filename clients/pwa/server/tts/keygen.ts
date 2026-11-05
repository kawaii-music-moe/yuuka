/**
 * yuuka 本体と音声生成サーバーの署名の鍵（Ed25519）を作る。
 *
 *   npx tsx server/tts/keygen.ts                 # 新しい鍵の組を作る
 *   npx tsx server/tts/keygen.ts <TTS_SIGNING_KEY> # 今の秘密鍵から公開鍵を出し直す
 *
 * 秘密鍵は yuuka の config.yaml の `TTS_SIGNING_KEY` に、公開鍵は音声生成サーバーの `TTS_VERIFY_KEYS` に書く。
 * 本番と dev は別々に作り、`TTS_VERIFY_KEYS` に両方の公開鍵を並べる。
 */
import { generateKeyPairSync } from "node:crypto";
import { privateKeyFromSeed, rawPublicKey } from "./auth";

const existing = process.argv[2];
let seed: string;
if (existing) {
	seed = existing;
} else {
	const { privateKey } = generateKeyPairSync("ed25519");
	seed = Buffer.from(
		privateKey.export({ format: "jwk" }).d as string,
		"base64url",
	).toString("base64");
}
const publicKey = rawPublicKey(privateKeyFromSeed(seed));
if (!existing)
	console.log(
		`# yuuka の config.yaml（秘密鍵・人に見せない）\nTTS_SIGNING_KEY: "${seed}"\n`,
	);
console.log(`# 音声生成サーバーの TTS_VERIFY_KEYS に足す公開鍵\n${publicKey}`);
