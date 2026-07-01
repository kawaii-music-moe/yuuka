I now have all facts grounded from primary sources. I have complete, verified coverage of the classic generateContent format, the new Interactions API, streaming/SSE, all crate versions with dates and maintenance status, and the SSE crates. Here is my report.

---

# Gemini REST API & Rust Client Research (verified July 2026)

**Verification note:** Every version number, date, and API field below was confirmed against a fetched primary source (Google docs on ai.google.dev, or crates.io/GitHub). URLs cited at the end. Google's docs are served both as HTML and raw `.md.txt`; where the small fetch model conflated the new "Interactions API" with the classic API, I cross-checked against the structured REST reference pages.

## 0. Headline finding: a new "Interactions API" landed in 2026 (中→高)

The single most important thing for a formal decision: **Google introduced a new "Interactions API" that is now Generally Available and is the officially recommended path for all new projects** as of 2026. The classic `generateContent` API is now explicitly labeled **"previous / legacy"** but remains supported.

- Classic `generateContent` docs now carry a banner: *"This version of the page covers the previous generateContent API. We recommend using the new Interactions API for access to all the latest features and models."* (verified: https://ai.google.dev/gemini-api/docs/generate-content/get-started)
- Interactions overview: *"The Interactions API is now generally available. We recommend using this API... optimized for agentic workflows, state management, and the latest models."* (verified: https://ai.google.dev/gemini-api/docs)

This matters because **the two APIs have different JSON shapes and different function-calling conventions** (details below). Any Rust decision must first pick which API to target. Most existing Rust crates target the classic `generateContent` API; near-zero Rust ecosystem support exists yet for the new Interactions API.

## 1. Official Rust SDK — still does NOT exist (高)

Confirmed from the official libraries page (https://ai.google.dev/gemini-api/docs/libraries): the official Google GenAI SDK covers **Python, JavaScript/TypeScript, Go, Java, and C#** (GA since May 2025). **Rust is not among them.** No official Google Rust SDK for Gemini in 2026. All Rust options are community/unofficial.

## 2. Classic `generateContent` — function calling structure (高)

Verified from the REST reference (https://ai.google.dev/api/generate-content, https://ai.google.dev/api/caching for shared schema types).

**Request — tools & declarations.** JSON is camelCase:
- `tools[]` → each has `functionDeclarations[]`
- `FunctionDeclaration`: `name`, `description`, `parameters` (Schema = OpenAPI 3.0 subset). There is **also** a mutually-exclusive `parametersJsonSchema` (full JSON Schema Value), plus `response` / `responseJsonSchema` for declared return types.

**Tool config:**
- `toolConfig.functionCallingConfig` with `mode` and `allowedFunctionNames[]`.
- `mode` enum (all values verified from reference): **`MODE_UNSPECIFIED`** (do not use), **`AUTO`** (default — model decides), **`ANY`** (constrained to always emit a function call), **`NONE`** (no function calls), **`VALIDATED`** (model decides, but validates calls via constrained decoding).

**Multi-turn round-trip (verified from Content/Part schema, https://ai.google.dev/api/caching):**
- `Content` has `parts[]` and `role`. `role` must be `"user"` or `"model"`.
- `Part` is a oneof: `text`, `inlineData`, `functionCall`, `functionResponse`, `fileData`, `executableCode`, `codeExecutionResult` (and newer `toolCall`/`toolResponse`).
- Model returns a `functionCall` part: `{ name, args, id? }` (`args` is a JSON Struct).
- Client sends back a `functionResponse` part: `{ name, response, id? }` (`response` is a JSON Struct; also optional `parts[]`, `willContinue`, `scheduling`).
- **Role convention nuance:** although `Content.role` is documented as only `user`/`model`, the reference explicitly states *"The next conversation turn may contain a FunctionResponse with the Content.role 'function'."* So in practice a third role value `"function"` is used to carry the tool result back. (This is the classic v1beta convention.)

## 3. Streaming — `streamGenerateContent` + SSE (高)

Verified from https://ai.google.dev/api/generate-content:
- Endpoint: `POST https://generativelanguage.googleapis.com/v1beta/{model=models/*}:streamGenerateContent`
- SSE is enabled with the query param **`?alt=sse`** (e.g. `...:streamGenerateContent?alt=sse&key=$GEMINI_API_KEY`, curl with `--no-buffer`).
- The response body is *"a stream of GenerateContentResponse instances."* Each SSE `data:` line is a full `GenerateContentResponse` JSON with `candidates[].content.parts[].text` (deltas) and, typically on the final chunk, `usageMetadata`.
- Without `alt=sse`, the endpoint returns the chunks as a JSON array of `GenerateContentResponse` objects (not incrementally useful) — so `alt=sse` is the mode you want for true streaming.

**New Interactions API streaming (中):** uses `stream: true` in the request body and emits richer typed SSE events — verified event names include `interaction.created`, `step.start`, `step.delta`, `interaction.completed`, `error` (https://ai.google.dev/api/interactions-api). Resumption via `GET /v1beta/interactions/{id}?stream=true` + `last_event_id`.

## 4. New Interactions API structure (中→高)

Verified from https://ai.google.dev/api/interactions-api:
- `POST https://generativelanguage.googleapis.com/v1beta/interactions`
- Body: `model`, `input` (accepts a `string`, `Content`, `Content[]`, `Step[]`, or `Turn[]`), `tools[]`, `stream`.
- Tools are **discriminated by `type`**: `{"type":"function","name":..,"description":..,"parameters":{JSON Schema}}`; other types `code_execution`, `google_search`, `file_search`, `google_maps`, `mcp_server`.
- Function calling uses **step** objects: model emits `{type:"function_call", name, arguments, id}`; client replies with a `function_result` step, and continues the conversation via **`previous_interaction_id`** (server-side state) rather than resending full history.

This is a meaningfully different shape from classic (`input` vs `contents`, `type:"function"` vs `functionDeclarations`, `function_call`/`function_result` steps vs `functionCall`/`functionResponse` parts, server-side state via `previous_interaction_id`). The earlier confusion in my fetches was exactly this: several guide pages now default to Interactions-API syntax.

## 5. Rust community crates — verified versions & status

All from crates.io JSON API + GitHub, July 2026:

| Crate | Latest ver | Date | Downloads | Maintained? | FC / streaming |
|---|---|---|---|---|---|
| **`gemini-rust`** (flachesis) | **1.7.1** | 2026-01-17 | 36.3k (13.6k recent) | **Yes — active** (v1.6.x Dec 2025, v1.7.x Jan 2026; 208 commits, 72★) | **Yes** both — README lists "Function Calling & Tools" + "Streaming Responses"; targets Gemini 2.5/3 |
| **`adk-gemini`** (zavora-ai) | **1.0.0** | 2026-06-07 | 8.5k (5.9k recent) | **Yes — very active** (0.8→1.0 May–Jun 2026) | Yes — content gen, streaming, function calling, embeddings, batch, caching, Vertex |
| **`gemini-rs`** (Shuflduf) | **2.0.0** | 2025-06-11 | 21.2k | Stale-ish (no release in ~13 months; ~1k LOC) | Basic; smaller surface |
| **`google-generative-ai-rs`** (avastmick) | 0.3.4 | 2024-12-23 | 35.6k | **DEAD — repo archived 2025-07-16**; README: *"NO LONGER MAINTAINED... now compatible with the OpenAI API format, little point"* | Do not use |
| **`google-ai-rs`** (veecore) | 0.1.2 | — | low (9★) | Appears inactive | Streaming yes; FC unclear |
| **`genai`** (jeremychone, multi-provider) | 0.6.5 stable / 0.7.0-beta.8 | stable 2026-06-06; beta 2026-07-01 | **247k (93k recent)** | **Yes — very active** | Multi-provider (Gemini + OpenAI/Anthropic/etc.); good if you want provider abstraction |

Notes:
- `rust-genai` as a *crate name* returned an ambiguous Google-focused result (latest 0.3.1, 2026-04-20) that does **not** match jeremychone's project — his is published as **`genai`**. Treat the `rust-genai` crate name as unverified/separate; **confidence 低** on that one specific entry.
- **None of these crates target the new Interactions API** based on their docs (they all use the classic `generateContent` / `contents` shape). Interactions-API Rust support is effectively absent in the ecosystem as of July 2026.

**Production-usability assessment:**
- Most viable off-the-shelf: **`gemini-rust` 1.7.1** (broad feature set, active, function calling + streaming, tracks Gemini 3) or **`adk-gemini` 1.0.0** (freshest, ADK-oriented, very active). Both classic-API only.
- **`genai` 0.6.5** if you want a multi-provider abstraction and are willing to accept its opinionated normalized model.
- Avoid `google-generative-ai-rs` (archived) and treat `gemini-rs`/`google-ai-rs` as lower-priority.

## 6. Recommended hand-rolled reqwest + serde wrapper (高 on tooling versions)

A thin typed wrapper is a defensible choice given: (a) no official SDK, (b) all community crates lag the new Interactions API, (c) you retain control over exactly which API/version you target. The REST surface is small.

**Core stack (verified versions):**
- **`reqwest` 0.13.4** (stable, published 2026-05-25). Note: 0.13.x is the current line; 0.12.x is still widely used — check your other deps' compatibility. Enable features `json`, `stream`, and a TLS backend (`rustls-tls` recommended).
- **`serde` / `serde_json`** for typed request/response structs. Use `#[serde(rename_all = "camelCase")]` for classic `generateContent` (camelCase JSON) and snake_case for the Interactions API. Model `Part` as an untagged/adjacently-tagged enum for the oneof; `args`/`response` as `serde_json::Value` or a `Map`.
- **`tokio`** async runtime.

**SSE handling — two verified options:**
1. **`eventsource-stream` 0.2.3** (published 2022-02-17; extremely stable, 14M+ downloads). It's a `Stream<Item = Result<Event, _>>` adapter over a byte stream. Pattern: `reqwest::Response::bytes_stream()` → adapt into `eventsource_stream::Eventsource` → for each `Event`, `serde_json::from_str::<GenerateContentResponse>(&event.data)`. This is the lightest touch and pairs naturally with the `?alt=sse` endpoint.
2. **`reqwest-eventsource` 0.6.0** (published 2024-03-29; 9M+ downloads, same author). Higher-level: wraps a `reqwest::RequestBuilder` into an `EventSource` that yields `Event::Message`, with built-in reconnection/retry semantics. Slightly heavier; reconnection is more relevant for the Interactions API's resumable `last_event_id` streams than for one-shot `streamGenerateContent`.

Both are dual MIT/Apache-2.0. `eventsource-stream` is the minimal building block; `reqwest-eventsource` is built on top of it and adds the reqwest integration + retry. For a simple `streamGenerateContent?alt=sse` consumer, `eventsource-stream` over `bytes_stream()` is the leanest. For a client that also needs resumable Interactions-API streams, `reqwest-eventsource` (or manual `Last-Event-ID` handling) is worth the extra weight.

**Sketch of the streaming loop (classic API):**
- Build `POST .../models/{model}:streamGenerateContent?alt=sse` with `x-goog-api-key` header (preferred over `?key=` in URL to keep the key out of logs).
- `resp.bytes_stream()` → `.eventsource()` → iterate; parse each `event.data` as `GenerateContentResponse`; accumulate `candidates[0].content.parts[].text`; stop on the chunk carrying `finishReason` / `usageMetadata`.

**Function-calling loop (classic API):**
- Send `contents` + `tools[].functionDeclarations` (+ optional `toolConfig.functionCallingConfig.mode`).
- If a returned `Part` has `functionCall {name, args}`, execute locally, then append a new `Content { role: "function", parts: [{ functionResponse: { name, response } }] }` and re-POST the full `contents` history.

## Confidence summary
- 高: no official Rust SDK; classic `generateContent` FC/streaming field names, mode enum, SSE `alt=sse`; all crate/tooling version numbers and dates.
- 中→高: Interactions API is GA & recommended, its endpoint/`input`/`type:function`/`function_call`/`previous_interaction_id`/`stream` structure, generateContent now labeled "previous/legacy."
- 低: the specific `rust-genai` crate-name entry (ambiguous, do not rely on it — use `genai`).

## Sources fetched
- https://ai.google.dev/gemini-api/docs/libraries
- https://ai.google.dev/gemini-api/docs
- https://ai.google.dev/gemini-api/docs/generate-content/get-started
- https://ai.google.dev/gemini-api/docs/function-calling (and `?example=weather`, `.md.txt`)
- https://ai.google.dev/api/generate-content (and `.md.txt`, `#v1beta.Tool`, `#method:-models.streamgeneratecontent`)
- https://ai.google.dev/api/caching (Content/Part/FunctionCall/FunctionResponse/FunctionCallingConfig schema)
- https://ai.google.dev/api/interactions-api
- crates.io API: google-generative-ai-rs, gemini-rs, gemini-rust, adk-gemini, genai, rust-genai, eventsource-stream, reqwest-eventsource, reqwest
- github.com: flachesis/gemini-rust, avastmick/google-generative-ai-rs, veecore/google-ai-rs

**Bottom line for the decision:** No official Rust SDK exists. Decide first whether to target the legacy `generateContent` API (broad, stable, well-covered by `gemini-rust` 1.7.1 / `adk-gemini` 1.0.0) or the new recommended **Interactions API** (better long-term, but essentially zero Rust crate support today → a hand-rolled reqwest+serde wrapper with `eventsource-stream` 0.2.3 for SSE is the pragmatic path if you want Interactions-API + full type control).