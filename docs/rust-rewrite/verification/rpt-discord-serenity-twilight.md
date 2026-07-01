I now have all facts verified against primary sources. Here is my report.

---

# Rust Discord Libraries: serenity vs twilight (verified July 2026)

Every version number and date below was confirmed against **crates.io's own API** (`crates.io/api/v1/...`, authoritative primary source) and cross-checked against docs.rs and GitHub tags. Note: crates.io's HTML pages are JS-rendered and return nothing to a fetcher, so I used its JSON API — those are the numbers to trust.

## 1. serenity (serenity-rs/serenity)

**Version & recency — 高 confidence**
- **Latest: 0.12.5, published 2025-12-20** (crates.io API `created_at: 2025-12-20T17:00:26Z`; confirmed by docs.rs header and GitHub tag).
- Prior: 0.12.4 (2024-11-15), 0.12.3 (2024-11-12), 0.12.2 (2024-06-01). So there was a ~13-month gap between 0.12.4 and 0.12.5.
- **Actively maintained in 2026 — 高.** The `current` branch has commits into 2026; the most recent visible is **2026-05-13** ("Fix Clippy and CI on current", #3549). ~455k downloads to date, tens of thousands weekly through mid-2026. MSRV 1.74.
- Note: 0.12.5 release notes call it "the last release for the 0.12.x series" — the next feature work targets a `next` branch, so releases are infrequent even though the repo is alive.

**Architecture — 高**
- Framework-style, **batteries-included, one `Client` per bot token.** You build a `Client`, attach an `EventHandler` (trait with async callbacks like `message`, `interaction_create`), and the library owns the event loop. Twilight's own homepage explicitly contrasts serenity as "an opinionated, batteries-included approach."
- Sharding is **transparent/automatic**: docs say "The `Shard` is transparently handled by the library... Sharded connections are automatically handled for you." A built-in `Cache` auto-updates from gateway events. Internally a `ShardManager` spawns/supervises shards; `Client` owns one, so you don't create a `ShardManager` yourself.
- **Multi-token / many-Clients-in-one-process concern — 中.** serenity is designed around **one Client = one token = one gateway identity**. Running many tokens means constructing many independent `Client`s. This is not a documented/blessed pattern. Each `Client` by default gets its **own `Arc<Cache>` and `Arc<Http>`**; the intended scaling story is "one shard/instance per process" (serenity's docs advise starting separate processes for other shard IDs), not many logical bots in one process. It is *technically possible* (you can hand each Client its own token, and each holds `cache: Arc<Cache>` / `http: Arc<Http>`), but per-Client cache is a real memory/resource multiplier at N tenants, and the framework's owned event loop gives you less control over N concurrent lifecycles.

**Button/component interactions — 高.** Full first-class support: `builder::CreateButton`, `CreateActionRow`, `CreateSelectMenu`, `CreateInteractionResponse`, `CreateInteractionResponseMessage`; models `ComponentInteraction`, `ComponentInteractionData`/`Kind`; plus a `ComponentInteractionCollector`. You handle these via `EventHandler::interaction_create`.

## 2. twilight (twilight-rs/twilight)

**Versions & recency — 高 confidence** (all from crates.io API):
| Crate | Latest | Published |
|---|---|---|
| twilight-gateway | **0.17.1** | 2025-12-13 |
| twilight-http | **0.17.1** | 2025-12-13 |
| twilight-model | **0.17.1** | 2025-12-13 |
| twilight-cache-inmemory | **0.17.1** | 2025-12-13 |
| twilight-util | **0.17.0** | 2025-11-08 |

- The 0.17.0 line landed 2025-11-08; a 0.17.1 point release across the core crates landed 2025-12-13. **twilight-util is genuinely still 0.17.0** (it got no 0.17.1 bump — verified in the versions JSON, not an oversight on my part). MSRV 1.79.
- **Active in 2026 — 高.** Downloads run into mid-2026; 0.17.x is recent. Smaller absolute download numbers than serenity (~43k/mo for the gateway crate) — it's the power-user library, not the beginner default.

**Modular design & what each crate does — 高**
Twilight is deliberately **à la carte** ("the library you use when you want—or, for scaling reasons, need—the freedom to structure things how you want"). Core crates compose rather than wrap:
- **twilight-model** — pure data types: all Discord API structs/enums (events, `Interaction`, `Component`/`Button`, and the `http::interaction::{InteractionResponse, InteractionResponseType, InteractionResponseData}` types). No I/O; shared vocabulary for the others.
- **twilight-gateway** — the WebSocket layer. Primary type `Shard` = a stateful connection maintaining one gateway session. Helper constructors `create_recommended`, `create_iterator`, `create_bucket` build many shards at once (reusing TLS context + session queue). Includes the IDENTIFY rate-limit `Queue`/`InMemoryQueue`.
- **twilight-http** — REST client. `InteractionClient` (via `Client::interaction(app_id)`) exposes `create_response(interaction_id, token, &InteractionResponse)` etc. Supports proxying so many services can share ratelimit budget.
- **twilight-cache-inmemory** — an *optional*, standalone cache you feed events into. Not implicit — if you don't want caching (or want per-tenant caching), you simply don't use it.
- **twilight-util** — opt-in helpers: builders like `InteractionResponseDataBuilder`, permission calculators, snowflake/timestamp utils, etc.
- (also `twilight-standby`, `twilight-gateway-queue`, `twilight-validate`, `twilight-mention`.)

**Multiple tokens / multiple gateway connections in one process — 高, and this is twilight's strong suit**
- Because gateway, http, model, and cache are **decoupled and mostly stateless-to-you**, there is no "Client-owns-everything" object forcing one identity per process. A `Shard` is just a value you own and poll. Running N tenants = holding N shards (or N groups of shards), each identified with its own token, each polled in its own `tokio` task.
- Twilight explicitly documents a **multi-serviced approach**: "a service that only connects shards to the gateway and sends the events to a broker to be processed," and as bots grow "multiple instances... groups of shards can be managed by each." The session `Queue` "can be composed to support multiple processes." This same flexibility is exactly what lets you keep many independent gateway sessions in **one** process cleanly.
- You control resource sharing explicitly: share one `twilight-http` `Client` (or one per tenant), share or omit caches per tenant, etc. — versus serenity where each `Client` drags its own cache/http by default.

**Reconnection resilience — 高, with an important nuance for your "self-recovery" requirement**
- The `Shard` **does** handle reconnect *and* resume logic internally — **but only while the caller keeps polling it.** Verified quotes from `docs.rs/twilight-gateway/.../struct.Shard.html`: *"Shards start out disconnected, but will on the first successful call to `poll_next` try to reconnect to the gateway,"* and *"`poll_next` must then be repeatedly called in order for the shard to maintain its connection and update its internal state."*
- So it is **caller-driven, not autonomous background reconnection.** The idiomatic pattern is a per-shard loop (`while let Some(item) = shard.next_event(...).await` / `next_message`), where **errors are yielded as `Result` items you log-and-continue on** rather than terminating the loop — the shard then reconnects/resumes on the next poll. The recommended shape wraps this in `tokio::spawn` per shard with a `tokio::select!` shutdown arm.
- Practical implication: twilight gives you the reconnect/resume machinery but **you own the supervision loop.** For a multi-tenant service that's actually an advantage — you can wrap each tenant's shard task with your own restart/backoff/health policy, which is harder when serenity owns the loop for you.

**Button/component interactions — 高.** Handled via twilight-model + twilight-http: receive `Interaction` (type `MessageComponent`) from the gateway, build a response with `InteractionResponseType::{ChannelMessageWithSource, UpdateMessage, DeferredUpdateMessage, Modal, ...}` and `InteractionResponseData` (has a `components` field; buttons carry `style`, `label`, `custom_id`), optionally via `twilight-util`'s `InteractionResponseDataBuilder`, then send with `InteractionClient::create_response(id, token, &response)`.

## 3. Verdict for "many tokens in one process, resilient, buttons" — 高 confidence: **twilight**

- **Multi-tenant fit:** twilight is the community- and docs-favored choice for anything non-monolithic. Its homepage self-selects for "scaling reasons... freedom to structure things how you want"; its guide has a dedicated *Multi-Serviced Approach* chapter. serenity's model is one Client/one token/one owned event loop with a per-Client cache — workable for many tokens but against the grain, with per-tenant memory overhead and less lifecycle control.
- **Resilience/self-recovery:** twilight's `Shard` does reconnect+resume, but requires you to drive the poll loop — which lets you attach per-tenant backoff/supervision. That's the right tradeoff for a multi-tenant supervisor. serenity auto-manages it but hides the loop, making per-tenant recovery policy harder.
- **Buttons:** both fully support component interactions; not a differentiator.
- **Resource control:** twilight lets you share one HTTP client and choose per-tenant caching (or none); serenity defaults each Client to its own cache+http.

**Real-world evidence — 中 confidence (transparency note):** twilight's *own* documentation clearly describes and endorses gateway/processing separation and multi-shard, multi-instance topologies (`twilight.rs/chapter_2_multi-serviced_approach`, gateway-queue repo for multi-process IDENTIFY coordination). I did **not** surface a specific first-hand blog post or GitHub issue of someone running many *different bot tokens* (as opposed to many shards of one token) in a single process — my targeted searches returned mostly discord.js/discord.py multi-account threads, not Rust ones. So the "many distinct tokens in one process" pattern is well-*supported* by twilight's architecture and documented multi-service philosophy, but I could not verify a public case study of that exact configuration. Treat the architectural suitability as 高, the "others have done exactly this and blogged it" as unverified.

### Sources fetched
- crates.io API (versions/dates): `https://crates.io/api/v1/crates/serenity/versions`, `.../twilight-gateway/versions`, `.../twilight-util/versions`
- docs.rs latest-version headers: `docs.rs/serenity`, `docs.rs/twilight-gateway`, `docs.rs/twilight-http`, `docs.rs/twilight-model`, `docs.rs/twilight-cache-inmemory`, `docs.rs/twilight-util`
- Reconnection semantics: `docs.rs/twilight-gateway/latest/twilight_gateway/struct.Shard.html`, `api.twilight.rs/twilight_gateway/struct.Shard.html`, `docs.rs/twilight-gateway/latest/twilight_gateway/`
- Serenity architecture/components: `docs.rs/serenity/latest/serenity/`, `docs.rs/serenity/latest/serenity/all.html`
- GitHub tags/commits: `github.com/serenity-rs/serenity/tags`, `.../commits/current`, `github.com/twilight-rs/twilight/tags`
- Philosophy/multi-service: `twilight.rs/`, `twilight.rs/chapter_2_multi-serviced_approach`
- Interaction/component API: search hits under `api.twilight.rs/twilight_model/http/interaction/*` and `twilight_http/client/struct.InteractionClient.html`

**Data-quality caveats:** crates.io HTML is JS-rendered and unreadable to a fetcher — I relied on its JSON API. Some fetches via the summarizer initially mislabeled years (e.g., "Dec 2024" for what the API confirms is 2025-12-20); where sources conflicted I deferred to the crates.io API `created_at` timestamps. twilight-gateway's README fetch returned truncated/generic content, so the reconnection quotes come from the Shard struct docs page instead.