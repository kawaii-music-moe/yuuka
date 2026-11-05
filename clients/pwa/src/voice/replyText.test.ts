// `npm test`（tsx --test）で実行する。
import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { FIRST_SENTENCE_CHARS, MAX_SENTENCE_CHARS, speakableText, speechSentences } from './replyText'

describe('speakableText', () => {
  it('drops markdown syntax and keeps the words', () => {
    assert.equal(speakableText('## 今日の予定\n- **10時** 定例\n- [資料](https://example.com/a) を確認'), '今日の予定\n10時 定例\n資料 を確認')
  })

  it('replaces code blocks, tables and bare URLs with short notes', () => {
    assert.match(speakableText('次を実行します。\n```bash\nls -la\n```'), /コードは画面で確認してください/)
    assert.match(speakableText('| 項目 | 金額 |\n|---|---|\n| 電気 | 5000 |'), /表は画面で確認してください/)
    assert.equal(speakableText('詳しくは https://example.com/x を見てください'), '詳しくは リンク を見てください')
  })

  it('keeps only the emojis the TTS model understands', () => {
    assert.equal(speakableText('できました😊🎉'), 'できました😊')
  })
})

describe('speechSentences', () => {
  it('splits on sentence endings and merges very short pieces', () => {
    assert.deepEqual(speechSentences('はい。今日は晴れです。明日は雨の予報なので傘を持っていってください。'), ['はい。今日は晴れです。', '明日は雨の予報なので傘を持っていってください。'])
  })

  it('keeps every sentence within the length limit', () => {
    const long = `${'あいうえおかきくけこ、'.repeat(12)}おわり。`
    for (const sentence of speechSentences(long)) assert.ok([...sentence].length <= MAX_SENTENCE_CHARS, sentence)
  })

  it('splits a long first sentence at a comma so the first audio starts sooner', () => {
    const [first] = speechSentences('明日の午前十時から、本社の会議室で四半期の定例ミーティングがあります。')
    assert.ok([...first].length <= FIRST_SENTENCE_CHARS)
    assert.equal(first, '明日の午前十時から、')
  })

  it('returns nothing for a reply without speakable text', () => {
    assert.deepEqual(speechSentences('🎉🎉'), [])
  })
})
