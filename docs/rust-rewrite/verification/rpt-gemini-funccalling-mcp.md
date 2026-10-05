I have thoroughly verified all the key facts from primary sources. I have enough to write a comprehensive, well-cited report. Let me compile the findings.

---

# Gemini Function Calling & Dynamic Tool Registration — Factual Report (July 2026)

## Important framing: there are now TWO Gemini API surfaces

As of mid-2026 there are two coexisting API surfaces, and the field names differ between them. Conflating them is the single biggest correctness risk for a plugin/registry design.

- **Classic `generateContent` API** (`v1beta`) — camelCase fields (`functionDeclarations`, `functionCall`, `args`, `functionResponse`, `toolConfig`, `functionCallingConfig`, `allowedFunctionNames`). Still fully supported. This is the surface almost all SDKs and third-party libraries target today.
- **Interactions API** — launched in **preview Dec 11, 2025**, went **GA in June 2026**, now Google's recommended interface. Uses a different, snake_case, step-based shape (`function_call` / `function_result` steps, `tool_choice`, `allowed_tools`, `previous_interaction_id`, server-side state). ([changelog](https://ai.google.dev/gemini-api/docs/changelog), [interactions-overview](https://ai.google.dev/gemini-api/docs/interactions-overview), [tools](https://ai.google.dev/gemini-api/docs/tools))

I give both below. Confidence that both surfaces exist and the GA/preview dates: **high**.

---

## 1. Function-calling API shape (classic `generateContent`)

Verified against the REST reference ([ai.google.dev/api/caching → FunctionDeclaration/Tool/ToolConfig](https://ai.google.dev/api/caching)) and the [function-calling doc](https://ai.google.dev/gemini-api/docs/function-calling). Confidence: **high**.

**Request:** `tools[]`, each `Tool` containing `functionDeclarations[]`.

`FunctionDeclaration` fields (verbatim from the REST reference):
- `name` (string, required) — "Must be a-z, A-Z, 0-9, or contain underscores, colons, dots, and dashes, with a maximum length of 128."
- `description` (string, required)
- `parameters` (`Schema`) — OpenAPI-3.0.3-subset schema.
- `parametersJsonSchema` (`Value`) — **full JSON Schema**; "mutually exclusive with `parameters`."
- `response` (`Schema`) — optional output schema.
- `responseJsonSchema` (`Value`) — full JSON Schema output; mutually exclusive with `response`.
- `behavior` (enum) — "Currently only supported by the BidiGenerateContent method" (i.e., Live/Bidi API).

**Which JSON Schema dialect / subset (critical for your sanitizing constraint):**

There are **two paths**, and this distinction is the crux of your plugin design:

- **`parameters` (the `Schema` type)** = a **subset of OpenAPI 3.0.3**. `Type` enum: `STRING`, `NUMBER`, `INTEGER`, `BOOLEAN`, `ARRAY`, `OBJECT`, `NULL`. Supported keywords: `type`, `format`, `title`, `description`, `nullable`, `enum`, `items`, `properties`, `required`, `minItems`, `maxItems`, `minLength`, `maxLength`, `pattern`, `minimum`, `maximum`, `propertyOrdering`, `prefixItems`. This path historically did **NOT** accept `additionalProperties`, `$ref`, `$defs` and rejects unknown keys with `Invalid JSON payload received. Unknown name "additionalProperties" at 'tools[0].function_declarations[0].parameters'`. ([caching ref](https://ai.google.dev/api/caching), [structured-output](https://ai.google.dev/gemini-api/docs/structured-output), [litellm#14330](https://github.com/BerriAI/litellm/issues/14330))
- **`parametersJsonSchema` (the `Value`/JSON-Schema path)** = **standard/full JSON Schema** (draft 2020-12 style). It "support[s] more fields such as `$ref`, `additionalProperties`, `prefixItems`, and `$defs`, and are directly sent to the backend so new fields added will be immediately available." `additionalProperties` support on the backend landed **November 2025**. ([python-genai#1815](https://github.com/googleapis/python-genai/issues/1815), [gemini-cli#4387](https://github.com/google-gemini/gemini-cli/issues/4387))

This is the single most important fact for you: **use `parametersJsonSchema` for arbitrary plugin schemas** — it accepts near-standard JSON Schema and is forwarded to the backend, so you need far less sanitizing than the legacy `parameters` path. Confidence: **high**.

Caveats even on the JSON-Schema path (confidence: **medium**, from forum/issue reports rather than a single canonical "unsupported list"):
- `default` is not in the supported-keyword list for the OpenAPI `parameters` path; model "ignores unsupported properties" there.
- `oneOf`/`allOf` are unreliable (`anyOf` is the supported union). ([oneOf thread](https://discuss.ai.google.dev/t/oneof-in-response-schema/55926))
- Some `additionalProperties` shapes still fail with `MALFORMED_FUNCTION_CALL` even via `parameters_json_schema`. ([discuss #119910](https://discuss.ai.google.dev/t/function-call-tool-with-additionalproperties-in-parameters-json-schema-fail-with-malformed-function-call/119910))
- `$schema` at the root is rejected — strip it. ([qwen-code#1186](https://github.com/QwenLM/qwen-code/issues/1186))
- "Very large or deeply nested schemas may be rejected." ([structured-output](https://ai.google.dev/gemini-api/docs/structured-output))

**Response / execution flow (classic):**
1. Model returns a `Content` part containing a **`functionCall`** with fields `name`, `args` (a Struct), and now an `id`. Parallel calls = multiple `functionCall` parts in one response.
2. App executes.
3. App sends back a part containing a **`functionResponse`** with fields `name`, `response` (Struct), `id` (to correlate with the call), plus `parts[]` (for **multimodal** function responses — Gemini 3), `willContinue`, and `scheduling`. ([caching ref](https://ai.google.dev/api/caching))

The per-call **`id`** correlation field was formally added in the **March 17, 2026** tooling update to support async/parallel dispatch. ([blog](https://blog.google/innovation-and-ai/technology/developers-tools/gemini-api-tooling-updates/)) Confidence: **high**.

**Interactions API equivalents** (snake_case, step-based): tool declared with `type: "function"`, `name`, `description`, `parameters`; model emits a `function_call` step (`type`, `name`, `arguments`, `id`); app returns a `function_result` step (`type`, `name`, `call_id`, `result[]` as typed content blocks). Streaming args arrive as `step.delta` events. ([function-calling doc](https://ai.google.dev/gemini-api/docs/function-calling), [interactions-overview](https://ai.google.dev/gemini-api/docs/interactions-overview)) Confidence: **high** on shape, **medium** on exact per-field names since the doc renders the newer surface and some fields weren't quotable verbatim.

---

## 2. `toolConfig` / `functionCallingConfig` modes

Classic API ([caching ref](https://ai.google.dev/api/caching)):
- `ToolConfig` = `{ functionCallingConfig, retrievalConfig, includeServerSideToolInvocations }`.
- `FunctionCallingConfig` = `{ mode, allowedFunctionNames[] }`.
- **`mode` enum: `AUTO`, `ANY`, `NONE`, `VALIDATED`.**
  - `AUTO` — model decides (default).
  - `ANY` — model is forced to emit a function call; constrain the set with `allowedFunctionNames[]`.
  - `NONE` — function calls prohibited.
  - **`VALIDATED` (Preview)** — model "ensures function schema adherence" (constrained decoding against the declared schema). This is newer than the classic three modes. ([function-calling doc](https://ai.google.dev/gemini-api/docs/function-calling))

Interactions API equivalent: `generation_config.tool_choice` with `"auto" | "any" | "none" | "validated"`, and `allowed_tools: { mode, tools: [...] }` to restrict the callable set. Confidence: **high**.

**Parallel function calling:** supported — multiple independent `functionCall` parts in one response (documented example calls three functions at once). **Compositional / sequential (chained) calling:** supported — the model chains dependent calls (e.g., get forecast → then set thermostat). Note the mechanism is still: model emits call(s) → you execute → you feed results back → model emits the next call. With the classic API you loop `generateContent`; with the Interactions API this is managed as steps within one interaction / via `previous_interaction_id`. ([function-calling doc](https://ai.google.dev/gemini-api/docs/function-calling)) Confidence: **high**.

**Gemini-3-only extras:** `streamFunctionCallArguments` (stream partial call args as generated) and multimodal `functionResponse` parts. Historical `ANY`-mode reliability complaints existed on 1.5-era models but are largely a non-issue on 2.5/3.x. ([gemini-3 guide](https://ai.google.dev/gemini-api/docs/gemini-3)) Confidence: **medium-high** (feature-flag names appear in secondary docs; not re-quotable from the top-level function-calling doc).

---

## 3. Native MCP support — YES, confirmed at two levels

This is real and important. Confidence: **high**.

**(a) REST/API-native.** The `Tool` object in the REST reference includes a **`mcpServers[]`** field: *"mcpServers[] object (McpServer) — Optional. MCP Servers to connect to."* Full `Tool` field list: `functionDeclarations[]`, `googleSearchRetrieval`, `codeExecution`, `googleSearch`, `computerUse`, `urlContext`, `fileSearch`, **`mcpServers[]`**, `googleMaps`. ([caching ref](https://ai.google.dev/api/caching)) The Interactions API also exposes a remote MCP tool type (`type: "mcp_server"` with `name`, `url`, `headers`, `allowed_tools`). ([tools](https://ai.google.dev/gemini-api/docs/tools))

**(b) SDK-native (client-side).** The Google Gen AI SDKs (`google-genai` Python/JS) let you pass a **local MCP `ClientSession` directly into `config.tools`**; the SDK calls the MCP server's `list_tools()`, converts them to `FunctionDeclaration`s, and runs **automatic function calling** (looping tool calls for you, bounded by `AutomaticFunctionCallingConfig`, e.g. `maximum_remote_calls`). This is still labeled **experimental** in the SDK. ([python-genai](https://github.com/googleapis/python-genai), [SDK docs](https://googleapis.github.io/python-genai/)) Confidence: **high** that it exists and is experimental; **medium** on exact current field names.

Origin: Google publicly committed to MCP at I/O 2025 and shipped it into the API/SDK thereafter. ([The New Stack](https://thenewstack.io/google-embraces-mcp/))

One caveat worth flagging: the top-level `changelog` and `tools` overview pages did not surface MCP prominently in the fetched content, while the REST reference and SDK clearly do. I rate the REST `mcpServers[]` field **high confidence** because it came verbatim from the API reference, but you should confirm `McpServer`'s exact sub-fields (transport, url, headers, auth) against the live `ai.google.dev/api/caching#McpServer` page before coding against it. **This is the one field I'd re-verify.**

---

## 4. Current models (mid-2026) and tool support

From [changelog](https://ai.google.dev/gemini-api/docs/changelog) and [models](https://ai.google.dev/gemini-api/docs/models). Confidence: **high** on IDs/dates; **medium** on the exhaustive per-model tool matrix (the models page doesn't tabulate FC per model).

Release timeline (verbatim dates):
- `gemini-3-pro-preview` — **Nov 18, 2025** (first Gemini 3 model).
- `gemini-3-flash-preview` — **Dec 17, 2025**.
- `gemini-3.1-pro-preview` — **Feb 19, 2026**.
- `gemini-3.5-flash` — **GA May 19, 2026**.

Current model IDs listed on the models page (mid-2026):
- **Gemini 3:** `gemini-3.5-flash` (Stable), `gemini-3.1-flash-lite` (Stable), `gemini-3.1-pro-preview` (Preview), `gemini-3-flash-preview` (Preview). (Also image variants: `gemini-3-pro-image-preview`, `gemini-3.1-flash-image-preview`.)
- **Gemini 2.5:** `gemini-2.5-pro`, `gemini-2.5-flash`, `gemini-2.5-flash-lite` (all Stable).

Per-model tool differences:
- All 2.5 and 3.x models support function calling. Gemini 3 + 2.5 use internal "thinking" that improves FC quality.
- **Gemini 3-only:** combining built-in tools + custom function calling in one request (tool combination, GA'd around the Mar 17, 2026 update), Maps grounding, `streamFunctionCallArguments` (Gemini 3 Pro+), multimodal `functionResponse` parts, and **thought signatures** — with stateless/classic requests you must echo back the model's thought-signature blocks in subsequent turns to preserve reasoning across function-calling rounds. ([gemini-3 guide](https://ai.google.dev/gemini-api/docs/gemini-3), [blog](https://blog.google/innovation-and-ai/technology/developers-tools/gemini-api-tooling-updates/))
- Deprecation flag: `gemini-2.5-flash-lite-preview-09-2025` scheduled shutdown **Mar 31, 2026**.

Note on 3.1/3.5 naming: `gemini-3.1-pro-preview` and `gemini-3.5-flash` clearly exist; I did not find a `gemini-3.5-pro` or `gemini-3.1-flash` (non-lite) confirmed — treat those as unverified.

---

## 5. Design pattern for a dynamic, heterogeneous tool registry

This synthesizes the verified facts into an architecture for your Rust plugin system (WASM / MCP / native traits). The API facts are cited; the architecture itself is my recommendation (confidence: **high** it's sound and API-compatible; it is design guidance, not a Google-documented pattern).

**Core model — a `ToolProvider` trait + central registry:**

```
trait ToolProvider {
    fn list(&self) -> Vec<ToolSpec>;                 // name, description, json_schema
    async fn invoke(&self, name, args: Value) -> Result<Value /*or multimodal parts*/>;
}
```
Backends: `WasmProvider`, `McpProvider` (wraps an MCP client, `list_tools()`→specs), `NativeProvider` (built-in Rust traits). The registry keeps `HashMap<String, Arc<dyn ToolProvider>>` name→provider, enforcing globally unique names (namespace-prefix per source, e.g. `mcp__github__create_issue`, to avoid collisions and stay within the 128-char, `[a-zA-Z0-9_:.-]` `name` constraint).

**Per-request declaration generation:** On each request, iterate providers → build `functionDeclarations[]` (or Interactions `tools[]`). Because tools/config are **interaction-scoped and must be re-sent every request even with `previous_interaction_id`**, dynamic regeneration each turn is the intended pattern, not an anti-pattern. ([interactions-overview](https://ai.google.dev/gemini-api/docs/interactions-overview))

**Schema handling — this is where your sanitizing constraint lives:**
- Prefer emitting each tool's schema into **`parametersJsonSchema`** (full JSON Schema, forwarded to backend, supports `$ref`/`$defs`/`additionalProperties`/`prefixItems`). This minimizes lossy conversion of arbitrary plugin schemas.
- Still run a **sanitizer pass** because real-world failures persist: strip `$schema`; drop or transform `default`; collapse `oneOf`/`allOf`→`anyOf` where possible; guard `additionalProperties` edge cases (some still 400 / `MALFORMED_FUNCTION_CALL`); flatten excessive nesting (deep/large schemas get rejected). Libraries already do exactly this (e.g. LibreChat's "strip unsupported JSON Schema keywords for Gemini MCP tools"). ([LibreChat#13850](https://github.com/danny-avila/LibreChat/pull/13850), [python-genai#1815](https://github.com/googleapis/python-genai/issues/1815), [discuss#119910](https://discuss.ai.google.dev/t/function-call-tool-with-additionalproperties-in-parameters-json-schema-fail-with-malformed-function-call/119910))
- If you must target the legacy `parameters` (OpenAPI) path, the sanitizer must down-convert to the STRING/NUMBER/INTEGER/BOOLEAN/ARRAY/OBJECT/NULL subset and drop `$ref`/`$defs`/`additionalProperties` (inline `$ref`s, etc.).

**Dispatch:** Parse each returned `functionCall` (`name`, `args`, `id`) → look up provider by `name` in the registry → `invoke` → build `functionResponse` (`name`, `response`, matching `id`; use `parts[]` for multimodal results on Gemini 3). Preserve the **`id`** for correct correlation under parallel calls. Loop until the model stops emitting calls (classic API) or let the Interactions API / SDK automatic-function-calling drive the loop (bound it with a max-call counter to avoid the well-documented `ANY`-mode infinite-loop failure mode). ([discuss loop thread](https://discuss.ai.google.dev/t/infinite-tool-call-loop-when-setting-function-calling-config-to-any-mode/97307))

**Two ways to wire MCP specifically:**
- **Delegate to Google:** pass MCP servers via the REST **`mcpServers[]`** tool field (server-side) or via the SDK's direct `ClientSession` tool (client-side auto-FC). Least code, but you lose per-call control and it's SDK-experimental.
- **Own the loop (recommended for a heterogeneous registry):** treat MCP as just another `ToolProvider` — you call `list_tools`, you convert schemas, you dispatch. This gives uniform handling across WASM/MCP/native, uniform sanitizing, and consistent `id`/namespace management. The trade-off is you reimplement what the SDK's auto-FC gives you for free.

---

## Confidence summary & flags

- **High:** classic FunctionDeclaration/Tool/ToolConfig field names; `parameters` vs `parametersJsonSchema` (OpenAPI-subset vs full JSON Schema, mutually exclusive); mode enum `AUTO/ANY/NONE/VALIDATED`; parallel + compositional calling; `functionCall`/`functionResponse` fields incl. `id`; existence of native MCP (`mcpServers[]` in REST + SDK `ClientSession`); model IDs and release dates; Interactions API GA June 2026.
- **Medium:** exact Interactions-API snake_case field names (docs rendered the new surface, not all fields quotable verbatim); the exhaustive unsupported-keyword list (assembled from issues/forums, not one canonical page); `streamFunctionCallArguments`/multimodal-response exact flag names.
- **Flag / re-verify before coding:** the exact sub-fields of the REST **`McpServer`** object (transport/url/headers/auth) — confirmed the field exists, but did not extract its full schema; check `https://ai.google.dev/api/caching#McpServer` live. Also confirm whether `VALIDATED` mode is GA or still Preview for your target model, and whether `gemini-3.5-pro`/`gemini-3.1-flash` (non-lite) exist (not found).

**Sources:**
- https://ai.google.dev/gemini-api/docs/function-calling
- https://ai.google.dev/api/caching (FunctionDeclaration, Tool, ToolConfig, FunctionCallingConfig, FunctionCall, FunctionResponse, McpServer)
- https://ai.google.dev/gemini-api/docs/structured-output
- https://ai.google.dev/gemini-api/docs/tools
- https://ai.google.dev/gemini-api/docs/interactions-overview
- https://ai.google.dev/gemini-api/docs/gemini-3
- https://ai.google.dev/gemini-api/docs/models
- https://ai.google.dev/gemini-api/docs/changelog
- https://blog.google/innovation-and-ai/technology/developers-tools/gemini-api-tooling-updates/
- https://github.com/googleapis/python-genai • https://googleapis.github.io/python-genai/
- https://github.com/googleapis/python-genai/issues/1815 • https://github.com/google-gemini/gemini-cli/issues/4387
- https://github.com/BerriAI/litellm/issues/14330 • https://github.com/danny-avila/LibreChat/pull/13850
- https://discuss.ai.google.dev/t/function-call-tool-with-additionalproperties-in-parameters-json-schema-fail-with-malformed-function-call/119910
- https://thenewstack.io/google-embraces-mcp/