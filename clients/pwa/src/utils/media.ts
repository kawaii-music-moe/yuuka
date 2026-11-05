// ブラウザでのファイル読み込み・画像縮小（送信前の前処理）。

/** Blob を base64（data URL の本体部分）にする。 */
export function readAsBase64(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(typeof reader.result === 'string' ? reader.result.split(',')[1] ?? '' : '')
    reader.onerror = () => reject(new Error('ファイルを読み込めませんでした。'))
    reader.readAsDataURL(blob)
  })
}

/**
 * 画像を長辺 `maxEdge` px 以下の JPEG に縮小する。HEIC など、ブラウザが描画できない形式は `null`
 * （呼び出し側で元のファイルをそのまま使う）。
 */
export async function downscaleImage(file: Blob, maxEdge: number, quality = 0.85): Promise<Blob | null> {
  let bitmap: ImageBitmap
  try {
    bitmap = await createImageBitmap(file)
  } catch {
    return null
  }
  const scale = Math.min(1, maxEdge / Math.max(bitmap.width, bitmap.height))
  const canvas = document.createElement('canvas')
  canvas.width = Math.round(bitmap.width * scale)
  canvas.height = Math.round(bitmap.height * scale)
  canvas.getContext('2d')?.drawImage(bitmap, 0, 0, canvas.width, canvas.height)
  bitmap.close()
  return new Promise((resolve) => canvas.toBlob((blob) => resolve(blob), 'image/jpeg', quality))
}
