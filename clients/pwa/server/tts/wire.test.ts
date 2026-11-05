// `npm test`（tsx --test）で実行する。
import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { decodeSpeaker, encodeSpeaker, floatToPcm16, parseSpeakRequest, pcm16ToFloat, ServerTtsError } from './wire'

describe('parseSpeakRequest', () => {
  it('accepts designed and saved voices', () => {
    assert.deepEqual(parseSpeakRequest({ text: 'はい。', voice: { kind: 'design', caption: '落ち着いた', voiceSeed: 2 }, seed: 1, durationScale: 1.1 }), {
      text: 'はい。', voice: { kind: 'design', caption: '落ち着いた', voiceSeed: 2 }, seed: 1, durationScale: 1.1,
    })
    const clone = parseSpeakRequest({ text: 'はい。', voice: { kind: 'clone', caption: '', key: 'k1' }, seed: 0, durationScale: 1 })
    assert.equal(clone.voice.kind, 'clone')
  })

  it('rejects bad voices and clamps the speed', () => {
    assert.throws(() => parseSpeakRequest({ text: 'はい。', voice: { kind: 'clone' } }), ServerTtsError)
    assert.throws(() => parseSpeakRequest({ text: 'あ'.repeat(401), voice: { kind: 'design', voiceSeed: 0 } }), ServerTtsError)
    assert.equal(parseSpeakRequest({ text: 'はい。', voice: { kind: 'design', voiceSeed: 0 }, durationScale: 9 }).durationScale, 1)
  })
})

describe('speaker and PCM conversion', () => {
  it('round-trips a speaker condition', () => {
    const speaker = { state: Float32Array.from({ length: 768 * 2 }, (_, i) => i / 10), mask: [true, true] }
    const { stateBase64, mask } = encodeSpeaker(speaker)
    assert.deepEqual([...decodeSpeaker(stateBase64, mask).state], [...speaker.state])
    assert.throws(() => decodeSpeaker(stateBase64, [true]), ServerTtsError)
  })

  it('converts float audio to 16 bit PCM and back', () => {
    const back = pcm16ToFloat(floatToPcm16(Float32Array.of(0, 0.5, -0.5, 1.5)))
    assert.deepEqual([...back].map((v) => Math.round(v * 100) / 100), [0, 0.5, -0.5, 1])
  })
})
