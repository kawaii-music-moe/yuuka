// `npm test`（tsx --test）で実行する。
import assert from 'node:assert/strict'
import { sign } from 'node:crypto'
import type { IncomingHttpHeaders } from 'node:http'
import { describe, it } from 'node:test'
import { canonicalMessage, HEADER, isLoopback, parseVerifyKeys, privateKeyFromSeed, rawPublicKey, RequestVerifier, sha256Hex } from './auth'
import { ServerTtsError } from './wire'

// 本体（`crates/yuuka-client-api/src/tts_proxy.rs` のテスト）と同じ値。Ed25519 の署名は決定的なので、
// 両方で同じ署名になることで、本体の署名をここで確かめられることを保証する。
const VECTOR = {
  seed: 'AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=',
  publicKey: 'A6EHv/POEL4dcN0Y50vAmWfk1jCbpQ1fHdyGZBJVMbg=',
  timeMs: '1762300000000',
  nonce: 'AAECAwQFBgcICQoLDA0ODw',
  body: '{"text":"はい。"}',
  bodySha256: '1e92f7713c5b643a5b11d074e0a309cc2e9d28b5e6d84608560d7f1b4321ce1b',
  signature: 'qu2BWar/M/pR0mxaJBXJP3ovcHH+8rnyFeXhkhDCPUpDuEXEsSkItPfpQcjAjw2drbyze1kGUW1Poo3btYmwAg==',
}

const key = privateKeyFromSeed(VECTOR.seed)
const NOW = Number(VECTOR.timeMs)

function signed(method: string, path: string, body: string, { timeMs = VECTOR.timeMs, nonce = VECTOR.nonce } = {}): IncomingHttpHeaders {
  const bodySha256 = sha256Hex(Buffer.from(body))
  return {
    [HEADER.time]: timeMs,
    [HEADER.nonce]: nonce,
    [HEADER.bodySha256]: bodySha256,
    [HEADER.signature]: sign(null, canonicalMessage(method, path, timeMs, nonce, bodySha256), key).toString('base64'),
  }
}

const rejects = (f: () => unknown) => assert.throws(f, (e) => e instanceof ServerTtsError && e.status === 401)

describe('署名の検証', () => {
  it('本体と同じ鍵・署名になる（固定の値）', () => {
    assert.equal(rawPublicKey(key), VECTOR.publicKey)
    assert.equal(sha256Hex(Buffer.from(VECTOR.body)), VECTOR.bodySha256)
    assert.deepEqual(signed('POST', '/speak', VECTOR.body)[HEADER.signature], VECTOR.signature)
  })

  it('正しい署名を通し、本文のハッシュも確かめる', () => {
    const verifier = new RequestVerifier(parseVerifyKeys(VECTOR.publicKey), () => NOW)
    const expected = verifier.verifyHeaders('POST', '/speak', signed('POST', '/speak', VECTOR.body))
    verifier.checkBody(expected, Buffer.from(VECTOR.body))
    rejects(() => verifier.checkBody(expected, Buffer.from('{"text":"いいえ。"}')))
  })

  it('同じ nonce の送り直しを断る', () => {
    const verifier = new RequestVerifier(parseVerifyKeys(VECTOR.publicKey), () => NOW)
    const headers = signed('GET', '/status', '')
    verifier.verifyHeaders('GET', '/status', headers)
    rejects(() => verifier.verifyHeaders('GET', '/status', headers))
  })

  it('時刻が 30 秒を超えてずれた要求を断る', () => {
    const verifier = new RequestVerifier(parseVerifyKeys(VECTOR.publicKey), () => NOW + 31_000)
    rejects(() => verifier.verifyHeaders('GET', '/status', signed('GET', '/status', '')))
    const ok = new RequestVerifier(parseVerifyKeys(VECTOR.publicKey), () => NOW + 29_000)
    ok.verifyHeaders('GET', '/status', signed('GET', '/status', ''))
  })

  it('別の操作・メソッド・鍵の署名と、署名の無い要求を断る', () => {
    const verifier = new RequestVerifier(parseVerifyKeys(VECTOR.publicKey), () => NOW)
    rejects(() => verifier.verifyHeaders('POST', '/clone', signed('POST', '/speak', VECTOR.body)))
    rejects(() => verifier.verifyHeaders('GET', '/speak', signed('POST', '/speak', VECTOR.body)))
    rejects(() => verifier.verifyHeaders('GET', '/status', {}))
    const other = rawPublicKey(privateKeyFromSeed(Buffer.alloc(32, 7).toString('base64')))
    rejects(() => new RequestVerifier(parseVerifyKeys(other), () => NOW).verifyHeaders('GET', '/status', signed('GET', '/status', '')))
  })

  it('並べた公開鍵のどれかで通す（本番と dev で別の鍵）', () => {
    const other = rawPublicKey(privateKeyFromSeed(Buffer.alloc(32, 7).toString('base64')))
    const verifier = new RequestVerifier(parseVerifyKeys(`${other}, ${VECTOR.publicKey}`), () => NOW)
    verifier.verifyHeaders('GET', '/status', signed('GET', '/status', ''))
  })

  it('期限の過ぎた nonce は忘れる', () => {
    let now = NOW
    const verifier = new RequestVerifier(parseVerifyKeys(VECTOR.publicKey), () => now)
    verifier.verifyHeaders('GET', '/status', signed('GET', '/status', ''))
    now += 61_000
    verifier.verifyHeaders('GET', '/status', signed('GET', '/status', '', { timeMs: String(now), nonce: 'BBECAwQFBgcICQoLDA0ODw' }))
    assert.equal((verifier as unknown as { nonces: Map<string, number> }).nonces.size, 1)
  })

  it('不正な鍵の指定と、待ち受けるアドレス', () => {
    assert.throws(() => parseVerifyKeys('AAAA'))
    assert.deepEqual(parseVerifyKeys(' '), [])
    assert.ok(isLoopback('127.0.0.1') && isLoopback('::1') && isLoopback('localhost'))
    assert.ok(!isLoopback('0.0.0.0') && !isLoopback('192.168.1.10'))
  })
})
