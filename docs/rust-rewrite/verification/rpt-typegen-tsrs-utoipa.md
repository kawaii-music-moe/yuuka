I have thorough, verified data on all three options plus the supporting research. I have enough primary-source material to write the report. Let me compile the final detailed report.

---

# Rust → TypeScript type generation: 2026 tooling comparison

All version/date/maintenance data below was pulled live (July 2026) from the crates.io API, npm registry, and GitHub API, then cross-checked against READMEs and docs.rs. Where a claim is inferred rather than directly quoted from a primary source, I flag it.

## Quick verdict

| | ts-rs | specta v2 (+ tauri-specta / rspc) | utoipa → openapi-typescript |
|---|---|---|---|
| Latest ver. | **12.0.1** (2026-01-31) | specta **2.0.0-rc.25** (2026-05-07) | utoipa **5.5.0** (2026-05-04); openapi-typescript **7.13.0** (2026-02-11) |
| Stable? | Yes | **No — RC** (docs.rs shows "pre-release" banner) | Yes (both) |
| Actively maintained? | Yes | Yes | Yes (both) |
| Scope | Types only | Types + functions/commands + events (via tauri-specta) | Full API contract (endpoints + req/resp) |
| Best fit for a plain Rust HTTP backend + Vite SPA | Strong | Weak (Tauri-oriented; rspc dead) | Strong |
| Confidence | **High** | **High** | **High** |

---

## Option 1 — ts-rs

**Versions / maintenance (High confidence)**
- Latest: **12.0.1**, released **2026-01-31** (12.0.0 same day; prior stable line 11.1.0 on 2025-10-14). Source: crates.io API.
- Repo `Aleph-Alpha/ts-rs`, **not archived**, ~1,830 stars, 32 open issues. Last commit **2026-06-17** ("Support `heapless::String` #494"); other 2026 commits include astrolabe support (2026-05-12) and `#[serde(deny_unknown_fields)]` on enums (2026-04-20). Clearly actively maintained. Source: GitHub API.
- ~9.9M all-time downloads.

**Feature set (High confidence — from README)**
- Emits **type-only** TypeScript declarations from Rust structs and enums; **generic types work**; Rust enums become **union declarations**.
- Generation is via a `#[derive(TS)]` macro. The idiomatic export path is **test-based**: annotate with `#[ts(export)]`, run `cargo test`, and types are written to `./bindings` (or `TS_RS_EXPORT_DIR`). There is also a programmatic API: `TS::export_all`, `TS::export`, `TS::export_to_string`.
- **Serde attributes explicitly supported**: `rename`, `rename_all`, `rename_all_fields`, `tag`, `content`, `untagged`, `skip`, `skip_serializing`, `skip_serializing_if`, `flatten`, `default`.

**Known limitations (High confidence)**
- `skip_serializing` / `skip_serializing_if` **only take effect when combined with `#[serde(default)]`** (so the generated type stays correct for both serialize and deserialize). This is the single most important gotcha for the fail-closed pattern (see below).
- `skip_deserializing` is **ignored**; use `#[ts(skip)]` to exclude a field that you can't `#[serde(skip)]`.
- Warns on unsupported serde attributes unless the `no-serde-warnings` feature is enabled.

**Fit for your use case:** This is the most direct match for "single source of truth for TS types" behind a Rust HTTP backend serving a Vite SPA. It generates types only (no endpoint contract), which is exactly the "shared types" slice.

---

## Option 2 — specta v2 (+ specta-typescript, tauri-specta, rspc)

**Versions / stability (High confidence)**
- **specta: 2.0.0-rc.25**, released **2026-05-07**. `max_stable_version` is still **1.0.5** — i.e. **v2 has never had a stable release; it is still a release candidate**. The RC line has been running for years (rc.20 was 2024-08, rc.21 in 2025-01), but is being actively worked. docs.rs for `2.0.0-rc.25` shows the explicit banner *"You are seeing a pre-release version of the specta crate."* Sources: crates.io API, docs.rs.
- **specta-typescript: 0.0.12** (2026-05-07) — note the `0.0.x` versioning; the TS exporter is the "stable"-labelled one in-README but carries a 0.0.x number.
- **tauri-specta: 2.0.0-rc.25** (2026-05-08); stable line stuck at 1.0.2. Also RC.
- Repo activity is **very high**: `specta-rs/specta` last commit **2026-07-01** ("upgrade everything", glam 0.33 compat, contributor guide); `specta-rs/tauri-specta` last commit **2026-07-01** (dependency upgrades, dependabot). Both healthy. Source: GitHub API.

**What specta adds over ts-rs (High confidence)**
- **Runtime type reflection**: the `Type` trait provides *runtime* type information and a `TypeCollection`/`Types` collection, versus ts-rs's compile-time-only emission. This is what lets it export function signatures and events.
- **Function/command export** via the `function` feature (`collect_functions!`), and **event export** via tauri-specta's `collect_events!`.
- Respects serde attributes (README example shows `#[serde(rename = "…")]` honored). It has a dedicated `specta-serde` format crate.
- Multi-language exporters exist (TS stable; Swift; Go/openapi/jsonschema/zod marked alpha/wip).

**rspc — the RPC-contract angle (High confidence)**
- **rspc is officially no longer maintained.** The README carries a banner: *"rspc is no longer being maintained. [Learn more]."* linking discussion #351.
- Discussion #351 is dated **2025-03-12**, by maintainer Oscar Beaumont. Reasons: none of his projects use it, Tauri integration is broken, docs are poor, and he's reallocating time. He recommends **no successor**; for Tauri work he says **tauri-specta is "generally good enough."** crates.io confirms rspc is frozen at `1.0.0-rc.5` / stable `0.4.1` (last release 2025-02-07); the repo's last real commit was **2025-11-24** (an axum 0.8 path fix), so it's essentially dormant.
- **End-to-end typed RPC contract:** rspc *did* provide this (tRPC-like), and it's the only piece of this ecosystem that gave you a true endpoint contract rather than just shared types. With rspc dead, **the specta ecosystem no longer offers a maintained end-to-end typed HTTP RPC solution.** tauri-specta gives typed commands/events but is **Tauri-IPC-specific**, not for a browser SPA talking to an HTTP backend.

**Bottom line for your stack:** specta shines for **Tauri** apps (typed commands + events). For a **Rust HTTP backend + browser Svelte/Vite SPA**, specta v2 buys you little over ts-rs for plain types, it's still RC (no stable release as of July 2026), and its RPC-contract story (rspc) is discontinued. I'd not pick this path for your described architecture.

---

## Option 3 — utoipa → OpenAPI 3.1 → openapi-typescript

**utoipa (High confidence)**
- Latest: **5.5.0**, released **2026-05-04** (5.4.0 was 2025-06-16). Repo `juhaku/utoipa`, ~3,912 stars, **not archived**, very active: last commit **2026-06-30** (time/cookie workaround; a breaking `feat!` removing serde_norway on 2026-06-19). ~33.7M downloads. Sources: crates.io + GitHub API.
- **OpenAPI 3.1**: utoipa implements OpenAPI Spec **v3.1** (search-confirmed; utoipa has shipped 3.1 since the v5 line). Medium confidence on exact wording, high confidence on the fact.
- **Framework integration**: first-class **axum** via the companion `utoipa-axum` crate (ergonomic router that registers handlers *and* emits the spec simultaneously) and `axum_extras`; **actix-web** via `actix_extras`. It's "code-first / compile-time generated" — you annotate handlers/DTOs with macros and derive `ToSchema`.

**openapi-typescript (High confidence)**
- Latest: **7.13.0**, released **2026-02-11** (`next` tag at 7.0.0-rc.1; `swagger-v2` legacy tag at 5.4.2). Companion runtime client **openapi-fetch 0.17.0** (2026-02-11). Repo `openapi-ts/openapi-typescript`, ~8,204 stars, not archived, actively maintained (releases through Feb 2026). Sources: npm registry + GitHub API.
- Converts **OpenAPI 3.0 and 3.1** schemas to TypeScript. It emits **types only, zero-runtime** (a `paths`/`components` type tree); the optional **openapi-fetch** package is a separate tiny typed fetch client that consumes those types for a fully-typed HTTP client. Mature, sponsored (Speakeasy), MIT.

**Fit for your use case:** This path gives you a true **API contract** — endpoints + request/response shapes — not just shared types. Because you already run Vite, `npx openapi-typescript ./openapi.json -o ./src/lib/api/schema.d.ts` slots in cleanly, and `openapi-fetch` gives end-to-end-typed calls in Svelte. This is the strongest choice if you want the *contract* (which your question explicitly notes ts-rs/specta don't give you). Trade-off: more moving parts (Rust macros → spec file → TS codegen) and OpenAPI's own type-mapping quirks.

---

## The fail-closed DTO / serde security pattern

**Idiomatic approaches (High confidence on the pattern; Medium on "which specific 2025-2026 posts"):**
- **Separate response DTO struct, distinct from the DB/domain model.** The response DTO only contains fields safe to expose; secret columns aren't fields on it at all, so they are **structurally impossible** to serialize into a response (true fail-closed by construction — you can't leak a field that doesn't exist on the type). The Rust forum thread on "serde serialize a struct in a different crate" reflects the common practice of keeping the domain model serde-free and implementing `Serialize` only on a DTO in the outer layer.
- **`#[serde(skip_serializing)]`** on a shared struct is the weaker, fail-*open*-by-omission variant: it works, but a new sensitive field added later is serialized unless someone remembers the attribute. Prefer the separate-DTO approach for anything secret.
- **`secrecy` crate** (`SecretString`/`SecretBox`): redacts `Debug`, zeroizes on drop, and only `expose_secret()` reveals the value; on deserialize errors it replaces the serde error to avoid leaking. This is the fail-closed primitive for secrets held in memory, complementary to the DTO boundary.
- **"Parse, don't validate" / newtype** pattern: wrap validated/parsed values in newtypes so invalid or sensitive raw data can't flow to the wire type.

**Do the generators respect these serde attributes? (High confidence)**
- **ts-rs**: yes for `#[serde(skip)]` and `skip_serializing` (with the important caveat that `skip_serializing`/`skip_serializing_if` only affect output when paired with `#[serde(default)]`; otherwise use `#[ts(skip)]`). **Best practice with any of these tools: use the separate-DTO approach so the secret field never exists on the exported type** — then the generated TS also cannot reference it, and drift/leaks are impossible by construction rather than by attribute discipline.
- **specta**: respects serde attributes (`rename` demonstrated; `specta-serde` format crate).
- **utoipa/openapi-typescript**: utoipa derives `ToSchema` from your DTO; a separate response DTO means the OpenAPI schema (and thus the TS) only ever contains safe fields.

---

## Vite integration + CI drift detection

**ts-rs (High confidence):**
- Integration: `#[ts(export)]` + `cargo test` writes `.ts` files into `./bindings` (configurable via `TS_RS_EXPORT_DIR`). Point Vite/tsconfig at that directory, or copy into `src/lib/`.
- **Drift CI recipe:** check the generated files into git, then in CI run the generation and fail if anything changed:
  ```bash
  cargo test export_bindings   # regenerates ./bindings
  git diff --exit-code -- ./bindings   # non-zero exit if stale
  ```
  This is the canonical "generate → `git diff --exit-code`" guard. (ts-rs discussion #328 explains the rationale for the `cargo test`-based generation approach.)

**specta / tauri-specta (High confidence on mechanism):** export runs from a builder in code/tests producing `bindings.ts`; same drift guard applies — regenerate in CI and `git diff --exit-code`. For a web (non-Tauri) app there's no clean, maintained runner today (rspc was it).

**utoipa → openapi-typescript (High confidence):**
- Emit the spec (utoipa `ApiDoc::openapi()` serialized to `openapi.json`, e.g. in a test/binary), then:
  ```bash
  npx openapi-typescript ./openapi.json -o ./src/lib/api/schema.d.ts
  ```
- **Drift CI:** two layers — regenerate `openapi.json` and `git diff --exit-code` it; and regenerate `schema.d.ts` and `git diff --exit-code` it. Optionally add openapi-fetch for a typed client. Wire the codegen as a Vite/npm `pregenerate` script or a CI step.

---

## Recommendation for *your* stack (Rust backend + Svelte 5 / Vite SPA)

- If you want **just shared types**, minimal moving parts, stable and actively maintained: **ts-rs** (12.0.1). High confidence.
- If you want a real **endpoint contract** (typed requests/responses per route, optional typed fetch client): **utoipa 5.5.0 → OpenAPI 3.1 → openapi-typescript 7.13.0 (+ openapi-fetch)**. High confidence. More setup, more payoff.
- **specta is not the right pick here**: v2 is still an RC (no stable release as of July 2026), its strengths are Tauri-specific, and its end-to-end RPC piece (**rspc**) was officially discontinued on 2025-03-12. High confidence.

**Not verified / caveats:** I could not open crates.io HTML pages (client-rendered) — all version data came from the crates.io **API** instead, which is authoritative. The exact prose asserting utoipa "full OpenAPI 3.1" came from search summaries of the utoipa README/docs rather than a direct quote (Medium confidence on wording, High on the fact). I did not find a single canonical 2025-2026 blog post dedicated to the "separate DTO fail-closed" pattern; the pattern is well-established and corroborated by the serde/secrecy sources cited, but treat "there's a definitive recent blog post" as unverified.

---

### Sources
- crates.io API (ts-rs, specta, specta-typescript, tauri-specta, rspc, utoipa) — live query 2026-07-01
- npm registry API (openapi-typescript, openapi-fetch) — live query 2026-07-01
- GitHub API repo + commits (Aleph-Alpha/ts-rs, specta-rs/specta, specta-rs/tauri-specta, specta-rs/rspc, juhaku/utoipa, openapi-ts/openapi-typescript) — live query 2026-07-01
- https://github.com/Aleph-Alpha/ts-rs (README)
- https://github.com/specta-rs/specta (README) and https://docs.rs/specta/2.0.0-rc.25/specta/
- https://github.com/specta-rs/rspc (README) and https://github.com/specta-rs/rspc/discussions/351
- https://github.com/specta-rs/tauri-specta ; https://github.com/specta-rs/website (tauri-specta v2 docs)
- https://github.com/juhaku/utoipa ; https://docs.rs/utoipa-axum
- https://openapi-ts.dev/
- https://docs.rs/secrecy ; https://users.rust-lang.org/t/is-there-a-better-way-to-serde-serialize-a-struct-defined-in-different-crate/98818
- https://github.com/Aleph-Alpha/ts-rs/discussions/328 (cargo-test-based generation rationale)