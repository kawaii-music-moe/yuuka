/** サーバーへ送るレシート画像（base64 本体と MIME）。 */
export type ReceiptImage = { imageBase64: string; mimeType: string }

/** サーバーが受け付けるレシート画像の形式（`ALLOWED_RECEIPT_MIME` と同じ）。 */
export const RECEIPT_MIME_TYPES = ['image/png', 'image/jpeg', 'image/jpg', 'image/webp', 'image/heic', 'image/heif', 'image/gif']

// スマホのカメラ写真は数 MB になり、base64 化するとリクエスト上限（10MB）を超えうる。
// OCR に十分な解像度まで縮めて JPEG で送る。
const MAX_EDGE = 2000
const JPEG_QUALITY = 0.85

function readAsBase64(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(typeof reader.result === 'string' ? reader.result.split(',')[1] ?? '' : '')
    reader.onerror = () => reject(new Error('画像を読み込めませんでした。'))
    reader.readAsDataURL(blob)
  })
}

async function downscale(file: File): Promise<Blob | null> {
  // HEIC など、ブラウザが描画できない形式は縮小せずにそのまま送る。
  let bitmap: ImageBitmap
  try {
    bitmap = await createImageBitmap(file)
  } catch {
    return null
  }
  const scale = Math.min(1, MAX_EDGE / Math.max(bitmap.width, bitmap.height))
  const canvas = document.createElement('canvas')
  canvas.width = Math.round(bitmap.width * scale)
  canvas.height = Math.round(bitmap.height * scale)
  canvas.getContext('2d')?.drawImage(bitmap, 0, 0, canvas.width, canvas.height)
  bitmap.close()
  return new Promise((resolve) => canvas.toBlob((blob) => resolve(blob), 'image/jpeg', JPEG_QUALITY))
}

/** 選択されたレシート画像を、送信用に縮小して base64 にする。 */
export async function prepareReceiptImage(file: File): Promise<ReceiptImage> {
  const resized = await downscale(file)
  if (resized) return { imageBase64: await readAsBase64(resized), mimeType: 'image/jpeg' }
  return { imageBase64: await readAsBase64(file), mimeType: file.type }
}
