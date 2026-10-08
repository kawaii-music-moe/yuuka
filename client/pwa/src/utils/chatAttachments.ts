// チャットの添付ファイル（画像・音声・動画・PDF・テキスト）の検証と送信用の変換。
import type { ChatAttachment } from '../api/contracts'
import { downscaleImage, readAsBase64 } from './media'

/** 1 回に添付できる数（サーバーの MAX_CHAT_ATTACHMENTS と同じ）。 */
export const MAX_CHAT_ATTACHMENTS = 4
/** 添付の合計サイズの上限。base64 で約 4/3 倍になってもリクエスト上限（10MB）に収まるようにする。 */
export const MAX_CHAT_ATTACHMENT_BYTES = 7 * 1024 * 1024
/** ファイル選択で選べる形式。 */
export const CHAT_ATTACHMENT_ACCEPT = 'image/*,audio/*,video/*,application/pdf,text/plain'

const IMAGE_MAX_EDGE = 2000

/** 送信できる形式か（サーバーの CHAT_ATTACHMENT_MIME_* と同じ）。 */
export const isSupportedChatAttachment = (mimeType: string) =>
  /^(image|audio|video)\//.test(mimeType) || ['application/pdf', 'text/plain'].includes(mimeType)

/** ファイルの種類（表示の出し分け用）。 */
export type MediaKind = 'image' | 'audio' | 'video' | 'file'
export const mediaKind = (mimeType: string): MediaKind =>
  mimeType.startsWith('image/') ? 'image' : mimeType.startsWith('audio/') ? 'audio' : mimeType.startsWith('video/') ? 'video' : 'file'

/**
 * 添付に追加できるかを確かめる。問題があれば案内文、なければ `null`。
 * `current` は既に添付済みのファイル、`adding` は今回選ばれたファイル。
 */
export function chatAttachmentError(current: Pick<File, 'size'>[], adding: Pick<File, 'type' | 'size'>[]): string | null {
  if (adding.some((file) => !isSupportedChatAttachment(file.type))) return '画像・音声・動画・PDF・テキストのファイルを選んでください。'
  if (current.length + adding.length > MAX_CHAT_ATTACHMENTS) return `添付は ${MAX_CHAT_ATTACHMENTS} 件までです。`
  const total = [...current, ...adding].reduce((sum, file) => sum + file.size, 0)
  if (total > MAX_CHAT_ATTACHMENT_BYTES) return '添付ファイルが大きすぎます（合計 7MB まで）。'
  return null
}

/** 送信用に変換する。静止画は長辺 2000px の JPEG に縮小する（GIF はアニメーションを保つためそのまま）。 */
export async function prepareChatAttachment(file: File): Promise<ChatAttachment> {
  if (file.type.startsWith('image/') && file.type !== 'image/gif') {
    const resized = await downscaleImage(file, IMAGE_MAX_EDGE)
    if (resized) return { name: file.name.replace(/\.[^.]+$/, '') + '.jpg', mimeType: 'image/jpeg', dataBase64: await readAsBase64(resized) }
  }
  return { name: file.name, mimeType: file.type, dataBase64: await readAsBase64(file) }
}
