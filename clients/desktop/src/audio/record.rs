//! マイク録音 → エンコード → base64（cpal / opus / ogg / hound）。
//!
//! 仕様: client_design.md §6 / requirements.md FR-6。
//!
//! ```text
//! [押下] cpal で既定入力デバイスを開く → PCM をリングバッファへ
//! [停止] PCM を Opus(20ms) → OGG 多重化 → base64 → WS {msg, audio:{mime:"audio/ogg"}}
//!        （重い/不安定環境は WAV(PCM16) フォールバック = hound）
//! ```
//!
//! 全実装は **`audio` feature の裏**にある。feature OFF（既定）でも
//! このモジュールはコンパイルでき、呼ぶと未対応エラーを返す。

use super::RecordedAudio;

/// 録音に関するエラー。
///
/// 各バリアントは feature 構成ごとに片方だけが使われる（audio OFF=`FeatureDisabled` /
/// audio ON=`Backend`）ため、未使用側の dead_code 警告を許容する。
#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum RecordError {
    /// `audio` feature が無効のままで録音が要求された。
    #[error("音声機能がこのビルドで無効です（--features audio で有効化）")]
    FeatureDisabled,
    /// 録音/エンコード処理中のエラー。
    #[error("recording error: {0}")]
    Backend(String),
}

/// 録音セッションのハンドル。
///
/// `start` で録音を開始し、`stop` で停止してエンコード済み [`RecordedAudio`] を得る。
/// 録音は別スレッド（cpal ストリームコールバック）で走り、UI はインジケータのみ。
pub struct Recorder {
    // feature 有効時のみ実体を持つ（cpal ストリーム等）。
    #[cfg(feature = "audio")]
    inner: imp::RecorderInner,
}

impl Recorder {
    /// 既定入力デバイスを開いて録音を開始する。
    ///
    /// `audio` feature 無効ビルドでは**実際には取り込まない**が、録音インジケータ UI を
    /// 確認できるよう開始自体は成功させる（`stop` で `FeatureDisabled` を返す）。
    pub fn start() -> Result<Self, RecordError> {
        #[cfg(feature = "audio")]
        {
            Ok(Recorder {
                inner: imp::RecorderInner::start()?,
            })
        }
        #[cfg(not(feature = "audio"))]
        {
            Ok(Recorder {})
        }
    }

    /// 録音を停止し、OGG/Opus（推奨）または WAV フォールバックで
    /// エンコードして base64 添付を返す。
    pub fn stop(self) -> Result<RecordedAudio, RecordError> {
        #[cfg(feature = "audio")]
        {
            self.inner.stop()
        }
        #[cfg(not(feature = "audio"))]
        {
            Err(RecordError::FeatureDisabled)
        }
    }
}

// ===========================================================================
// feature 有効時の実装（スケルトン）
// ===========================================================================

#[cfg(feature = "audio")]
mod imp {
    use std::sync::{Arc, Mutex};

    use base64::Engine;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    use super::{RecordError, RecordedAudio};

    /// 録音の上限（メモリ/送信サイズ保護。client_design.md §6: 例 60s）。
    const MAX_SECONDS: usize = 60;

    /// cpal 入力ストリーム＋蓄積バッファ。ストリームのコールバック（別スレッド）が
    /// `samples` へ i16 PCM を追記し、`stop` で停止・収集して WAV 化する。
    pub struct RecorderInner {
        stream: cpal::Stream,
        samples: Arc<Mutex<Vec<i16>>>,
        sample_rate: u32,
        channels: u16,
    }

    impl RecorderInner {
        pub fn start() -> Result<Self, RecordError> {
            let host = cpal::default_host();
            let device = host.default_input_device().ok_or_else(|| {
                RecordError::Backend("既定の入力デバイス（マイク）が見つかりません".into())
            })?;
            let supported = device
                .default_input_config()
                .map_err(|e| RecordError::Backend(format!("入力設定の取得に失敗: {e}")))?;
            let sample_format = supported.sample_format();
            let config: cpal::StreamConfig = supported.into();
            let sample_rate = config.sample_rate.0;
            let channels = config.channels;
            let max_samples = MAX_SECONDS * sample_rate as usize * channels.max(1) as usize;

            let samples = Arc::new(Mutex::new(Vec::<i16>::new()));
            let buf = samples.clone();
            let err_fn = |e| log::warn!("audio input stream error: {e}");

            // サンプル形式を i16 へ正規化しつつ追記（上限到達で打ち切り）。
            let stream = match sample_format {
                cpal::SampleFormat::F32 => device.build_input_stream(
                    &config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        push(&buf, data.iter().map(|&s| f32_to_i16(s)), max_samples)
                    },
                    err_fn,
                    None,
                ),
                cpal::SampleFormat::I16 => device.build_input_stream(
                    &config,
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        push(&buf, data.iter().copied(), max_samples)
                    },
                    err_fn,
                    None,
                ),
                cpal::SampleFormat::U16 => device.build_input_stream(
                    &config,
                    move |data: &[u16], _: &cpal::InputCallbackInfo| {
                        push(&buf, data.iter().map(|&s| u16_to_i16(s)), max_samples)
                    },
                    err_fn,
                    None,
                ),
                other => {
                    return Err(RecordError::Backend(format!(
                        "未対応のサンプル形式: {other:?}"
                    )))
                }
            }
            .map_err(|e| RecordError::Backend(format!("入力ストリーム生成に失敗: {e}")))?;

            stream
                .play()
                .map_err(|e| RecordError::Backend(format!("録音開始に失敗: {e}")))?;

            Ok(RecorderInner {
                stream,
                samples,
                sample_rate,
                channels,
            })
        }

        pub fn stop(self) -> Result<RecordedAudio, RecordError> {
            // ストリームを止めて（drop）からバッファを収集する。
            drop(self.stream);
            let pcm = {
                let mut g = self
                    .samples
                    .lock()
                    .map_err(|_| RecordError::Backend("録音バッファのロックに失敗".into()))?;
                std::mem::take(&mut *g)
            };
            if pcm.is_empty() {
                return Err(RecordError::Backend(
                    "録音データが空でした（マイク入力が無い可能性）".into(),
                ));
            }

            // WAV(PCM16) へエンコード（hound）。Gemini はネイティブで音声を解釈する。
            let spec = hound::WavSpec {
                channels: self.channels,
                sample_rate: self.sample_rate,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let mut cursor = std::io::Cursor::new(Vec::<u8>::new());
            {
                let mut writer = hound::WavWriter::new(&mut cursor, spec)
                    .map_err(|e| RecordError::Backend(format!("WAV 生成に失敗: {e}")))?;
                for s in pcm {
                    writer
                        .write_sample(s)
                        .map_err(|e| RecordError::Backend(format!("WAV 書込に失敗: {e}")))?;
                }
                writer
                    .finalize()
                    .map_err(|e| RecordError::Backend(format!("WAV 確定に失敗: {e}")))?;
            }
            let bytes = cursor.into_inner();
            let data_base64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            Ok(RecordedAudio {
                mime: "audio/wav".into(),
                data_base64,
            })
        }
    }

    /// コールバックから i16 サンプルを上限までバッファへ追記する。
    fn push(buf: &Arc<Mutex<Vec<i16>>>, it: impl Iterator<Item = i16>, max: usize) {
        if let Ok(mut g) = buf.lock() {
            for s in it {
                if g.len() >= max {
                    break;
                }
                g.push(s);
            }
        }
    }

    fn f32_to_i16(s: f32) -> i16 {
        (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
    }

    fn u16_to_i16(s: u16) -> i16 {
        (s as i32 - 32768) as i16
    }
}
