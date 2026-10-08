import { downscaleImage, readAsBase64 } from './media'

/** サーバーへ送るレシート画像（base64 本体と MIME）。 */
export type ReceiptImage = { imageBase64: string; mimeType: string }

/** サーバーが受け付けるレシート画像の形式（`ALLOWED_RECEIPT_MIME` と同じ）。 */
export const RECEIPT_MIME_TYPES = ['image/png', 'image/jpeg', 'image/jpg', 'image/webp', 'image/heic', 'image/heif', 'image/gif']

// スマホのカメラ写真は数 MB になり、base64 化するとリクエスト上限（10MB）を超えうる。
// OCR に十分な解像度まで縮めて JPEG で送る。
const MAX_EDGE = 2000

/** 選択されたレシート画像を、送信用に縮小して base64 にする。 */
export async function prepareReceiptImage(file: File): Promise<ReceiptImage> {
  const resized = await downscaleImage(file, MAX_EDGE)
  if (resized) return { imageBase64: await readAsBase64(resized), mimeType: 'image/jpeg' }
  return { imageBase64: await readAsBase64(file), mimeType: file.type }
}
