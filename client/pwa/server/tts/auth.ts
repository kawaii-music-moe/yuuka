/**
 * yuuka 本体からの要求の署名（Ed25519）を確かめる。本体（`crates/yuuka-client-api/src/tts_proxy.rs`）が秘密鍵で
 * 署名し、ここは公開鍵だけを持つ（音声生成サーバーを別のマシンに置いても、偽の要求は作れない）。
 *
 * 署名する文字列（UTF-8・改行区切り）:
 *
 *   yuuka-tts-v1 \n <METHOD> \n <path> \n <時刻 ms> \n <nonce> \n <本文の SHA-256（hex）>
 *
 * 本文のハッシュはヘッダーで先に受け取り、署名を確かめてから本文を読む（署名の無い要求は本文を読まずに断る）。
 * 時刻が ±30 秒を超えてずれた要求と、一度使った nonce は断る（送り直しの防止）。
 */
import { createHash, createPrivateKey, createPublicKey, timingSafeEqual, verify, type KeyObject } from 'node:crypto'
import type { IncomingHttpHeaders } from 'node:http'
import { ServerTtsError } from './wire'

export const SIGNATURE_VERSION = 'yuuka-tts-v1'
export const HEADER = { time: 'x-yuuka-tts-time', nonce: 'x-yuuka-tts-nonce', bodySha256: 'x-yuuka-tts-body-sha256', signature: 'x-yuuka-tts-signature' } as const
const MAX_SKEW_MS = 30_000
/** 覚えておく nonce の上限（正しい署名の要求だけが入るので、普段は数件）。 */
const MAX_NONCES = 10_000
const NONCE_PATTERN = /^[A-Za-z0-9_-]{16,64}$/
const SHA256_HEX = /^[0-9a-f]{64}$/

export function canonicalMessage(method: string, path: string, timeMs: string, nonce: string, bodySha256: string): Buffer {
  return Buffer.from([SIGNATURE_VERSION, method.toUpperCase(), path, timeMs, nonce, bodySha256].join('\n'), 'utf8')
}

export const sha256Hex = (body: Uint8Array) => createHash('sha256').update(body).digest('hex')

/** 生の 32 バイトの公開鍵（base64）から鍵を作る。 */
export function publicKeyFromBase64(raw: string): KeyObject {
  const bytes = Buffer.from(raw, 'base64')
  if (bytes.length !== 32) throw new Error('Ed25519 の公開鍵は 32 バイト（base64）で指定してください')
  return createPublicKey({ key: { kty: 'OKP', crv: 'Ed25519', x: bytes.toString('base64url') }, format: 'jwk' })
}

/** 32 バイトの秘密鍵（seed・base64。本体の `TTS_SIGNING_KEY`）から鍵を作る。 */
export function privateKeyFromSeed(seed: string): KeyObject {
  const bytes = Buffer.from(seed, 'base64')
  if (bytes.length !== 32) throw new Error('Ed25519 の秘密鍵は 32 バイト（base64）で指定してください')
  // PKCS#8 の Ed25519 秘密鍵は、決まった前置き 16 バイト + seed。
  return createPrivateKey({ key: Buffer.concat([Buffer.from('302e020100300506032b657004220420', 'hex'), bytes]), format: 'der', type: 'pkcs8' })
}

export const rawPublicKey = (key: KeyObject) => Buffer.from(key.export({ format: 'jwk' }).x as string, 'base64url').toString('base64')

/** `TTS_VERIFY_KEYS`（公開鍵の base64 をカンマか空白で区切る）を読む。本番と dev で別の鍵を使える。 */
export function parseVerifyKeys(value: string | undefined): KeyObject[] {
  return (value ?? '').split(/[\s,]+/).filter(Boolean).map(publicKeyFromBase64)
}

const header = (headers: IncomingHttpHeaders, name: string) => {
  const value = headers[name]
  return typeof value === 'string' ? value : undefined
}

const unauthorized = () => new ServerTtsError(401, '署名を確かめられませんでした')

export class RequestVerifier {
  /** nonce → 忘れてよい時刻。追加した順（≒ 時刻順）に並ぶ。 */
  private readonly nonces = new Map<string, number>()

  constructor(private readonly keys: KeyObject[], private readonly now: () => number = Date.now) {
    if (keys.length === 0) throw new Error('公開鍵がありません')
  }

  /**
   * 本文を読む前に、ヘッダーの署名を確かめる。通れば、本文の SHA-256（hex）を返すので、本文を読んでから
   * {@link checkBody} に渡す。
   */
  verifyHeaders(method: string, path: string, headers: IncomingHttpHeaders): string {
    const timeMs = header(headers, HEADER.time)
    const nonce = header(headers, HEADER.nonce)
    const bodySha256 = header(headers, HEADER.bodySha256)
    const signature = header(headers, HEADER.signature)
    if (!timeMs || !nonce || !bodySha256 || !signature) throw unauthorized()
    if (!/^\d{1,16}$/.test(timeMs) || !NONCE_PATTERN.test(nonce) || !SHA256_HEX.test(bodySha256)) throw unauthorized()
    const now = this.now()
    if (Math.abs(now - Number(timeMs)) > MAX_SKEW_MS) throw new ServerTtsError(401, '要求の時刻がずれています（時計を合わせてください）')
    const sig = Buffer.from(signature, 'base64')
    if (sig.length !== 64) throw unauthorized()
    const message = canonicalMessage(method, path, timeMs, nonce, bodySha256)
    if (!this.keys.some((key) => verify(null, message, key, sig))) throw unauthorized()
    // 署名が正しい要求の nonce だけを覚える（署名の無い要求で表を埋められないように）。
    this.forgetExpired(now)
    if (this.nonces.has(nonce) || this.nonces.size >= MAX_NONCES) throw unauthorized()
    this.nonces.set(nonce, now + 2 * MAX_SKEW_MS)
    return bodySha256
  }

  checkBody(expectedSha256: string, body: Uint8Array) {
    if (!timingSafeEqual(Buffer.from(sha256Hex(body), 'hex'), Buffer.from(expectedSha256, 'hex'))) throw unauthorized()
  }

  private forgetExpired(now: number) {
    for (const [nonce, expires] of this.nonces) {
      if (expires > now) break
      this.nonces.delete(nonce)
    }
  }
}

const LOOPBACK = new Set(['127.0.0.1', '::1', 'localhost'])
export const isLoopback = (host: string) => LOOPBACK.has(host) || host.startsWith('127.')
