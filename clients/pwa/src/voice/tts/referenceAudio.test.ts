// `npm test`（tsx --test）で実行する。期待値は audiotools（上流が使う実装）の出力。
import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { downmix, integratedLoudness, prepareReference, REFERENCE_SAMPLE_RATE } from './referenceAudio'

const sr = REFERENCE_SAMPLE_RATE
const sine = (f: number, amp: number, sec: number) => Float32Array.from({ length: Math.floor(sr * sec) }, (_, i) => amp * Math.sin((2 * Math.PI * f * i) / sr))
const peak = (x: Float32Array) => x.reduce((m, v) => Math.max(m, Math.abs(v)), 0)
const concat = (...parts: Float32Array[]) => {
  const out = new Float32Array(parts.reduce((n, p) => n + p.length, 0))
  let o = 0
  for (const p of parts) { out.set(p, o); o += p.length }
  return out
}

describe('integratedLoudness', () => {
  it('matches audiotools (BS.1770-4) within 0.01 LU', () => {
    const cases: [string, Float32Array, number][] = [
      ['1 kHz sine', sine(1000, 0.1, 3), -23.045],
      ['gated bursts', concat(sine(440, 0.5, 1), new Float32Array(sr * 2), sine(220, 0.05, 1)), -10.4621],
      ['shorter than 0.5 s', sine(1000, 0.2, 0.3), -19.0655],
      ['100 Hz', sine(100, 0.8, 2), -6.8129],
    ]
    for (const [name, x, expected] of cases) {
      const got = integratedLoudness(x)
      assert.ok(Math.abs(got - expected) < 0.01, `${name}: ${got} vs ${expected}`)
    }
  })

  it('floors silence at -70 LUFS', () => {
    assert.equal(integratedLoudness(new Float32Array(sr)), -70)
  })
})

describe('prepareReference', () => {
  it('normalizes to -16 LUFS like the upstream runtime', () => {
    const out = prepareReference(sine(1000, 0.1, 3))
    assert.ok(Math.abs(integratedLoudness(out) + 16) < 0.01)
    assert.ok(Math.abs(peak(out) - 0.22504) < 1e-3)
  })

  it('keeps the peak within 1.0', () => {
    const spiky = concat(sine(1000, 0.001, 2), Float32Array.of(0.9), sine(1000, 0.001, 2))
    assert.ok(peak(prepareReference(spiky)) <= 1.0000001)
  })
})

describe('downmix', () => {
  it('averages channels', () => {
    assert.deepEqual([...downmix([Float32Array.of(1, 0), Float32Array.of(0, 1)])], [0.5, 0.5])
  })
})
