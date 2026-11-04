/**
 * 音声入力（Web Speech API の SpeechRecognition）。1 回の発話を聞き取って文字にする。
 * Chrome では音声が Google のサーバーに送られて認識される。
 */

type RecognitionResultList = ArrayLike<ArrayLike<{ transcript: string }> & { isFinal: boolean }>
type Recognition = {
  lang: string
  interimResults: boolean
  continuous: boolean
  maxAlternatives: number
  onresult: ((event: { resultIndex: number; results: RecognitionResultList }) => void) | null
  onerror: ((event: { error: string }) => void) | null
  onend: (() => void) | null
  start(): void
  stop(): void
  abort(): void
}
type RecognitionConstructor = new () => Recognition

function recognitionClass(): RecognitionConstructor | undefined {
  const w = window as unknown as { SpeechRecognition?: RecognitionConstructor; webkitSpeechRecognition?: RecognitionConstructor }
  return w.SpeechRecognition ?? w.webkitSpeechRecognition
}

export const speechInputAvailable = () => typeof window !== 'undefined' && recognitionClass() !== undefined

export class SpeechInputError extends Error {
  constructor(public readonly code: string) {
    super(describe(code))
  }
}

function describe(code: string): string {
  switch (code) {
    case 'not-allowed':
    case 'service-not-allowed': return 'マイクの使用が許可されていません。ブラウザの設定でマイクを許可してください。'
    case 'audio-capture': return 'マイクが見つかりません。'
    case 'network': return '音声認識のサーバーに接続できません。'
    default: return `音声を聞き取れませんでした（${code}）。`
  }
}

/**
 * 1 回の発話を聞き取る。話している途中の文字は `onInterim` で受け取れる。
 * 何も聞き取れずに終わった場合は空文字を返す。`signal` で中断すると、そこまでの文字を返す。
 */
export function listenOnce(options: { onInterim?: (text: string) => void; signal?: AbortSignal } = {}): Promise<string> {
  const Recognition = recognitionClass()
  if (!Recognition) return Promise.reject(new SpeechInputError('unsupported'))
  const recognition = new Recognition()
  recognition.lang = 'ja-JP'
  recognition.interimResults = true
  recognition.continuous = false
  recognition.maxAlternatives = 1
  return new Promise((resolve, reject) => {
    let finalText = ''
    let interimText = ''
    let failure: SpeechInputError | undefined
    recognition.onresult = (event) => {
      interimText = ''
      for (let i = event.resultIndex; i < event.results.length; i++) {
        const result = event.results[i]
        if (result.isFinal) finalText += result[0].transcript
        else interimText += result[0].transcript
      }
      options.onInterim?.(finalText + interimText)
    }
    recognition.onerror = (event) => {
      // 無音で終わった・中断したは失敗ではない。
      if (event.error !== 'no-speech' && event.error !== 'aborted') failure = new SpeechInputError(event.error)
    }
    recognition.onend = () => {
      options.signal?.removeEventListener('abort', onAbort)
      if (failure) reject(failure)
      else resolve((finalText || interimText).trim())
    }
    const onAbort = () => recognition.stop()
    options.signal?.addEventListener('abort', onAbort, { once: true })
    recognition.start()
  })
}
