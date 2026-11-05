/** 音声合成 Worker（`worker.ts`）とのメッセージ。 */
import type { StageTimings } from './pipeline'

/**
 * 読み上げる声。`design` はキャプションと声の番号から作る声（Worker が初回だけ作ってキャッシュする）、
 * `clone` は声のサンプルから作った話者条件（`clone` リクエストの結果を端末に保存しておいたもの）。
 * どちらもキャプションは話し方の指定として使う（`clone` では空でもよい）。
 */
export type VoiceSpec =
  | { kind: 'design'; caption: string; voiceSeed: number }
  | { kind: 'clone'; caption: string; state: Float32Array; mask: boolean[]; key?: string }

export type WorkerRequest =
  /** 端末が対応しているか・モデルが保存済みかを調べる（ダウンロードはしない）。 */
  | { type: 'probe' }
  /** モデルを読み込む（未保存なら取得して保存する）。 */
  | { type: 'load' }
  /** 1 文を合成する。届いた順に処理する。`seed` は文ごとの抑揚の揺らぎ、`durationScale` は話す速さ（大きいほどゆっくり）。 */
  | { type: 'speak'; id: number; text: string; voice: VoiceSpec; seed: number; durationScale: number }
  /** 声のサンプル（48 kHz モノラル）から話者条件を作る。合成と同じ順番で処理する。 */
  | { type: 'clone'; id: number; audio: Float32Array }
  /** キャプションと声の番号から作る声の話者条件を返す（保存用）。 */
  | { type: 'design'; id: number; caption: string; voiceSeed: number }
  /** 処理中・待ち行列の合成をすべて取りやめる。 */
  | { type: 'cancel' }
  | { type: 'clear' }

export type WorkerResponse =
  | { type: 'probe'; supported: boolean; reason?: string; stored: boolean; totalBytes: number }
  | { type: 'progress'; loaded: number; total: number }
  | { type: 'ready'; adapter: string }
  | { type: 'chunk'; id: number; audio: Float32Array; sampleRate: number; final: boolean; timings?: StageTimings }
  /** `clone`・`design` の結果。 */
  | { type: 'speaker'; id: number; state: Float32Array; mask: boolean[] }
  /** 合成が終わった（打ち切り・空文を含む）。 */
  | { type: 'done'; id: number }
  | { type: 'error'; id?: number; message: string }
  | { type: 'cleared' }
