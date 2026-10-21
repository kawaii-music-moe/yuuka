//! `streamGenerateContent?alt=sse` の受信（SSE イベントの切り出しと、断片を 1 つの応答へまとめる処理）。
//!
//! まとめた応答は `generateContent` と同じ形で FC ループへ返す。本文の断片は連結し、functionCall・思考・
//! `thoughtSignature` 付きの part は届いた順にそのまま残す（Gemini 3 系は署名を次のリクエストで返さないと
//! 400 になるため、断片の付随フィールドを落とさない）。

use yuuka_core::GeminiError;

use crate::client::status_error;
use crate::wire::{
    Candidate, Content, ErrorEnvelope, GenerateContentResponse, Part, PromptFeedback, Role,
    UsageMetadata,
};

/// SSE のバイト列からイベントの `data` を切り出す。受信の区切りがイベントや行の途中でもよい。
#[derive(Default)]
pub(crate) struct SseDecoder {
    buf: Vec<u8>,
    data: String,
}

impl SseDecoder {
    /// 受け取ったバイト列を足し、空行で完結したイベントの `data` を届いた順に返す。
    pub(crate) fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(bytes);
        let mut events = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            // 改行（0x0A）は UTF-8 の多バイト文字の途中に現れないので、行単位なら安全に文字列化できる。
            let line = String::from_utf8_lossy(&line);
            if let Some(event) = self.line(line.trim_end_matches(['\r', '\n'])) {
                events.push(event);
            }
        }
        events
    }

    /// 末尾の空行が無いまま本文が終わったときに、残っているイベントを返す。
    pub(crate) fn finish(mut self) -> Option<String> {
        if !self.buf.is_empty() {
            let rest = std::mem::take(&mut self.buf);
            self.data_line(String::from_utf8_lossy(&rest).trim_end_matches('\r'));
        }
        (!self.data.is_empty()).then_some(self.data)
    }

    /// 1 行を処理する。空行ならそこまでの `data` をイベントとして確定する。
    fn line(&mut self, line: &str) -> Option<String> {
        if line.is_empty() {
            return (!self.data.is_empty()).then(|| std::mem::take(&mut self.data));
        }
        self.data_line(line);
        None
    }

    /// `data:` 行を今のイベントへ足す（コメント行 `:` や他のフィールドは使わない）。
    fn data_line(&mut self, line: &str) {
        if let Some(value) = line.strip_prefix("data:") {
            if !self.data.is_empty() {
                self.data.push('\n');
            }
            self.data.push_str(value.strip_prefix(' ').unwrap_or(value));
        }
    }
}

/// 受け取った断片を 1 つの応答へまとめる（先頭候補だけを見る・`GenerateContentResponse::text` と同じ）。
#[derive(Default)]
pub(crate) struct StreamAccumulator {
    parts: Vec<Part>,
    role: Option<Role>,
    finish_reason: Option<String>,
    saw_candidate: bool,
    usage_metadata: Option<UsageMetadata>,
    prompt_feedback: Option<PromptFeedback>,
    /// ここまでに届いた本文（思考 part を除く）。
    text: String,
}

impl StreamAccumulator {
    /// SSE イベント 1 件（`data` の JSON）を取り込む。本文が増えたら `true`。
    ///
    /// # Errors
    /// 本文の途中で届いたエラー（`{"error": …}`）は HTTP の失敗と同じ分類の [`GeminiError`]。JSON として
    /// 読めないイベントは [`GeminiError::Decode`]。
    pub(crate) fn push_event(&mut self, data: &str) -> Result<bool, GeminiError> {
        let value: serde_json::Value =
            serde_json::from_str(data).map_err(|source| GeminiError::Decode { source })?;
        if value.get("error").is_some() {
            let env: ErrorEnvelope = serde_json::from_value(value).unwrap_or_default();
            let status = env.error.as_ref().and_then(|e| e.code).unwrap_or(500);
            return Err(status_error(status, &env));
        }
        let chunk: GenerateContentResponse =
            serde_json::from_value(value).map_err(|source| GeminiError::Decode { source })?;
        Ok(self.push(chunk))
    }

    /// ここまでに届いた本文。
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// まとめた応答（候補が 1 つも届かなければ候補なし＝空応答）。
    pub(crate) fn into_response(self) -> GenerateContentResponse {
        let candidates = if self.saw_candidate {
            vec![Candidate {
                content: (!self.parts.is_empty()).then(|| Content {
                    role: self.role.unwrap_or(Role::Model),
                    parts: self.parts,
                }),
                finish_reason: self.finish_reason,
            }]
        } else {
            Vec::new()
        };
        GenerateContentResponse {
            candidates,
            usage_metadata: self.usage_metadata,
            prompt_feedback: self.prompt_feedback,
        }
    }

    fn push(&mut self, chunk: GenerateContentResponse) -> bool {
        if chunk.usage_metadata.is_some() {
            self.usage_metadata = chunk.usage_metadata;
        }
        if chunk.prompt_feedback.is_some() {
            self.prompt_feedback = chunk.prompt_feedback;
        }
        let Some(candidate) = chunk.candidates.into_iter().next() else {
            return false;
        };
        self.saw_candidate = true;
        if candidate.finish_reason.is_some() {
            self.finish_reason = candidate.finish_reason;
        }
        let Some(content) = candidate.content else {
            return false;
        };
        self.role = Some(content.role);
        let before = self.text.len();
        for part in content.parts {
            self.push_part(part);
        }
        self.text.len() > before
    }

    fn push_part(&mut self, part: Part) {
        if !is_plain_text(&part) {
            if !is_thought(&part) {
                if let Some(text) = &part.text {
                    self.text.push_str(text);
                }
            }
            self.parts.push(part);
            return;
        }
        let delta = part.text.unwrap_or_default();
        // 中身も付随フィールドも無い part は捨てる（空の text part を送り返すと 400 になりうる）。
        if delta.is_empty() {
            return;
        }
        self.text.push_str(&delta);
        match self.parts.last_mut().filter(|last| is_plain_text(last)) {
            Some(last) => last.text.get_or_insert_with(String::new).push_str(&delta),
            None => self.parts.push(Part::text(delta)),
        }
    }
}

/// 付随フィールドの無い本文だけの part か（連結してよい断片）。
fn is_plain_text(part: &Part) -> bool {
    part.text.is_some()
        && part.inline_data.is_none()
        && part.function_call.is_none()
        && part.function_response.is_none()
        && part.file_data.is_none()
        && part.extra.is_empty()
}

fn is_thought(part: &Part) -> bool {
    part.extra
        .get("thought")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_events_survive_split_lines_crlf_and_multibyte_boundaries() {
        let raw =
            "data: {\"a\":\"こんにちは\"}\r\n\r\ndata: {\"b\":1}\n\n: comment\n\ndata: {\"c\":2}";
        let bytes = raw.as_bytes();
        let mut decoder = SseDecoder::default();
        let mut events = Vec::new();
        // 1 バイトずつ流しても（多バイト文字の途中で切れても）同じイベントになる。
        for b in bytes {
            events.extend(decoder.feed(std::slice::from_ref(b)));
        }
        events.extend(decoder.finish());
        assert_eq!(
            events,
            vec![
                "{\"a\":\"こんにちは\"}".to_owned(),
                "{\"b\":1}".to_owned(),
                "{\"c\":2}".to_owned()
            ]
        );
    }

    #[test]
    fn multiple_data_lines_join_with_newline() {
        let mut decoder = SseDecoder::default();
        let events = decoder.feed(b"data: {\"a\":\ndata: 1}\n\n");
        assert_eq!(events, vec!["{\"a\":\n1}".to_owned()]);
        assert_eq!(decoder.finish(), None);
    }

    fn chunk(parts: &str) -> String {
        format!(r#"{{"candidates":[{{"content":{{"role":"model","parts":{parts}}}}}]}}"#)
    }

    #[test]
    fn accumulator_merges_text_and_keeps_signed_and_call_parts() {
        let mut acc = StreamAccumulator::default();
        assert!(acc.push_event(&chunk(r#"[{"text":"予定を"}]"#)).unwrap());
        assert_eq!(acc.text(), "予定を");
        assert!(acc
            .push_event(&chunk(r#"[{"text":"確認します"}]"#))
            .unwrap());
        assert!(!acc
            .push_event(&chunk(
                r#"[{"functionCall":{"name":"getSchedules","args":{}},"thoughtSignature":"sig-1"}]"#
            ))
            .unwrap());
        assert!(!acc
            .push_event(&chunk(r#"[{"text":"","thoughtSignature":"sig-2"}]"#))
            .unwrap());
        // 中身の無い断片は捨てる。
        assert!(!acc.push_event(&chunk(r#"[{"text":""}]"#)).unwrap());
        let last =
            r#"{"candidates":[{"finishReason":"STOP"}],"usageMetadata":{"totalTokenCount":42}}"#;
        assert!(!acc.push_event(last).unwrap());

        let resp = acc.into_response();
        assert_eq!(resp.text(), "予定を確認します");
        let candidate = &resp.candidates[0];
        assert_eq!(candidate.finish_reason.as_deref(), Some("STOP"));
        let parts = &candidate.content.as_ref().unwrap().parts;
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].text.as_deref(), Some("予定を確認します"));
        assert_eq!(
            parts[1].function_call.as_ref().unwrap().name,
            "getSchedules"
        );
        assert_eq!(parts[1].extra["thoughtSignature"], "sig-1");
        assert_eq!(parts[2].text.as_deref(), Some(""));
        assert_eq!(parts[2].extra["thoughtSignature"], "sig-2");
        assert_eq!(resp.usage_metadata.unwrap().total_token_count, Some(42));
    }

    #[test]
    fn accumulator_hides_thoughts_from_text() {
        let mut acc = StreamAccumulator::default();
        assert!(!acc
            .push_event(&chunk(r#"[{"text":"(考え中)","thought":true}]"#))
            .unwrap());
        assert!(acc
            .push_event(&chunk(r#"[{"text":"はい","thoughtSignature":"s"}]"#))
            .unwrap());
        assert!(acc.push_event(&chunk(r#"[{"text":"、どうぞ"}]"#)).unwrap());
        assert_eq!(acc.text(), "はい、どうぞ");
        let resp = acc.into_response();
        let parts = &resp.candidates[0].content.as_ref().unwrap().parts;
        assert_eq!(parts.len(), 3);
        assert_eq!(resp.text(), "はい、どうぞ");
    }

    #[test]
    fn accumulator_without_candidates_is_empty_response() {
        let acc = StreamAccumulator::default();
        let resp = acc.into_response();
        assert!(resp.candidates.is_empty());
        assert_eq!(resp.text(), "");
    }

    #[test]
    fn error_event_mid_stream_is_classified_like_http() {
        let mut acc = StreamAccumulator::default();
        let err = acc
            .push_event(r#"{"error":{"code":503,"status":"UNAVAILABLE","message":"busy"}}"#)
            .unwrap_err();
        assert!(matches!(err, GeminiError::ServerError { status: 503 }));
        assert!(err.is_retryable());
        let err = acc.push_event("not json").unwrap_err();
        assert!(matches!(err, GeminiError::Decode { .. }));
    }
}
