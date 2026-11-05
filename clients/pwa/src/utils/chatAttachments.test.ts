// `npm test`（tsx --test）で実行する。
import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import { chatAttachmentError, MAX_CHAT_ATTACHMENT_BYTES, mediaKind } from './chatAttachments'

const file = (type: string, size = 1000) => ({ type, size })

describe('chatAttachmentError', () => {
  it('accepts images, audio, video, PDF and text', () => {
    assert.equal(chatAttachmentError([], [file('image/png'), file('audio/mpeg'), file('video/mp4'), file('application/pdf')]), null)
    assert.equal(chatAttachmentError([], [file('text/plain')]), null)
  })

  it('rejects unsupported types, too many files and too large totals', () => {
    assert.match(chatAttachmentError([], [file('application/zip')]) ?? '', /画像・音声/)
    assert.match(chatAttachmentError([file('image/png'), file('image/png'), file('image/png')], [file('image/png'), file('image/png')]) ?? '', /4 件まで/)
    assert.match(chatAttachmentError([file('video/mp4', MAX_CHAT_ATTACHMENT_BYTES)], [file('image/png', 1)]) ?? '', /大きすぎ/)
  })
})

describe('mediaKind', () => {
  it('classifies by MIME type', () => {
    assert.deepEqual(['image/png', 'audio/ogg', 'video/mp4', 'application/pdf'].map(mediaKind), ['image', 'audio', 'video', 'file'])
  })
})
