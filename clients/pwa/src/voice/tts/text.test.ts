// `npm test`（tsx --test）で実行する。期待値は上流 `irodori_tts` の Python 実装の出力。
import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { createTokenizers, durationFeatures, normalizeText, UnigramTokenizer, type TokenizerJson } from './text'

describe('normalizeText', () => {
  it('matches the upstream normalization', () => {
    const cases: [string, string][] = [
      ['「こんにちは」', 'こんにちは'],
      ['テスト？！', 'テスト?!'],
      ['〜です...', 'ーです…'],
      ['（全体）を（囲む）', '(全体)を(囲む)'],
      ['ｶﾀｶﾅＡＢＣ', 'カタカナABC'],
      ['a\tb　c', 'abc'],
    ]
    for (const [input, expected] of cases) assert.equal(normalizeText(input), expected, input)
  })
})

describe('durationFeatures', () => {
  it('matches build_duration_features', () => {
    const got = durationFeatures('おはよう、ございます!今日は東京ABC😊…ー?。', 12, 256, false)
    const expected = [0.046875, 0.515823, 0.5, 0.315465, 0.244651, 0.315465, 0.315465, 0.315465, 0.315465, 0.315465, 0.458333, 0.166667, 0.125, 0]
    assert.equal(got.length, expected.length)
    got.forEach((v, i) => assert.ok(Math.abs(v - expected[i]) < 1e-5, `feature ${i}: ${v} vs ${expected[i]}`))
  })
})

// 小さな語彙で Viterbi・byte fallback・無効化・強制分割を確かめる（本物の語彙との一致は 415 文で確認済み）。
const vocab: [string, number][] = [
  ['<unk>', 0], ['<s>', 0], ['</s>', 0],
  ['東', -5], ['京', -5], ['東京', -3], ['都', -5], ['東京都', -9], ['▁', -2], ['a', -4], ['b', -4], ['ab', -6],
  ...Array.from({ length: 256 }, (_, b): [string, number] => [`<0x${b.toString(16).toUpperCase().padStart(2, '0')}>`, -10]),
]
const json: TokenizerJson = { model: { type: 'Unigram', unk_id: 0, byte_fallback: true, vocab } }
const id = (piece: string) => vocab.findIndex(([p]) => p === piece)

describe('UnigramTokenizer', () => {
  it('picks the highest-scoring segmentation', () => {
    const tok = new UnigramTokenizer(json)
    assert.deepEqual(tok.encode('東京都'), [id('東京'), id('都')]) // -3 + -5 > -9
    assert.deepEqual(tok.encode('ab'), [id('ab')]) // a + b = -8 より ab = -6 が高い
  })

  it('replaces spaces with ▁ and falls back to UTF-8 bytes for unknown characters', () => {
    const tok = new UnigramTokenizer(json)
    assert.deepEqual(tok.encode('a b'), [id('a'), id('▁'), id('b')])
    assert.deepEqual(tok.encode('猫'), [...new TextEncoder().encode('猫')].map((b) => id(`<0x${b.toString(16).toUpperCase()}>`)))
  })

  it('applies disabled tokens and forced splits only to reading text', () => {
    const { text, caption } = createTokenizers(json, { bos_token_id: 1, disabled: [id('東京')], forced: { [id('ab')]: [id('a'), id('b')] } })
    assert.deepEqual(text.encodeBody('東京'), [id('東'), id('京')])
    assert.deepEqual(caption.encodeBody('東京'), [id('東京')])
    assert.deepEqual(text.encodeBody('ab'), [id('a'), id('b')])
    const { ids, mask } = text.encode('東京', 2)
    assert.deepEqual([...ids], [1n, BigInt(id('東'))])
    assert.deepEqual([...mask], [1, 1])
  })
})
