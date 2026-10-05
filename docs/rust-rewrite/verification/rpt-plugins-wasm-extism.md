I have everything needed for a thorough, well-sourced report. Let me compile the final findings.

---

# Untrusted Rust Plugin Execution Foundations — 2026 Research Report

*Research date: July 2026. All version numbers/dates verified against crates.io API, GitHub releases, and official docs as noted. Confidence flagged per claim.*

## Executive summary

For running **untrusted** user-supplied plugins in a Rust backend, the security ranking is unambiguous in 2026:

1. **WASM (wasmtime or Extism)** — the only in-process option that gives a real capability sandbox. Extism = ergonomics + polyglot plugins; raw wasmtime = maximum control + the standards-track Component Model.
2. **Process isolation** (separate OS process + JSON-lines/MCP over stdio, hardened with seccomp/namespaces) — use when plugins need native libs or you want an OS/kernel-level boundary.
3. **Native `.so` loading (libloading / abi_stable)** — **not viable for untrusted code**. No sandbox whatsoever; a malicious plugin fully compromises the process/host.

---

## 1. wasmtime

### Version & release cadence (Confidence: HIGH)
- **Latest: `wasmtime 46.0.1`, published 2026-06-24** (crates.io `max_stable_version` = 46.0.1; `46.0.0` published 2026-06-22). Verified via crates.io API and GitHub releases.
- Wasmtime ships a **new semver-major every month** (~20th). LTS releases are versions that are **multiples of 12** (e.g., 24, 36, 48…), supported **24 months**; non-LTS releases supported ~2 months. Current maintained LTS branches receiving backports on 2026-06-24 included 24.x and 36.x.
- Source: [github.com/bytecodealliance/wasmtime/releases](https://github.com/bytecodealliance/wasmtime/releases), [crates.io API](https://crates.io/api/v1/crates/wasmtime), [Wasmtime LTS article](https://bytecodealliance.org/articles/wasmtime-lts), [release process docs](https://docs.wasmtime.dev/stability-release.html).

### Component Model & WASI status (Confidence: HIGH)
- **WASI 0.2 (Preview 2 / "WASI 0.2") is stable and production-ready** and has been since it first shipped; the WASI repo's 0.2.x line continues (e.g., `v0.2.11` on 2026-04-07, and wasmtime 46 notes WASI **0.2.12** support). The Component Model + WIT is the foundation. Sources: [wasi.dev](https://wasi.dev/), [Java Code Geeks WASM 2026](https://www.javacodegeeks.com/2026/04/webassembly-in-2026-where-it-has-landed-what-wasi-0-2-changes-and-why-java-and-kotlin-developers-should-pay-attention-now.html).
- **WASI 0.3 shipped 2026-06-11** (ratified by the WASI Subgroup). It rebases WASI onto the Component Model's **native async primitives** — `async func`, `stream<T>`, `future<T>` — moving the old `wasi:io` pollables/streams into the canonical ABI, with a single host-managed event loop shared across components. **Wasmtime 46 ships WASI 0.3.0 with `component-model-async` enabled by default** (0.3 RC ran on wasmtime 45; support in wasmtime 43+ and jco). **WASI 1.0 is expected late 2026 / early 2027.** Sources: [Bytecode Alliance: WASI 0.3 Launched](https://bytecodealliance.org/articles/WASI-0.3), [WASI v0.3.0 release](https://github.com/WebAssembly/WASI/releases/tag/v0.3.0), [wasmtime releases](https://github.com/bytecodealliance/wasmtime/releases).
- **Bottom line for a plugin backend:** As of mid-2026 you can build on the Component Model + WASI 0.2 as a stable, production foundation; WASI 0.3 async is now available if you need streaming/async host<->guest, but it's newer (treat as "fresh-stable").

### Host functions → guest (Confidence: HIGH)
- Host functions are exposed by defining them on a **`Linker`** and instantiating the guest against it; WASI itself is added via `wasmtime-wasi` into the `Linker`. Per-instance state lives in a **`Store`**. With the Component Model you define interfaces in **WIT** and generate typed host/guest glue (see tooling below). Sources: [Wasmtime embedding API docs](https://docs.wasmtime.dev/api/wasmtime/), [Store docs](https://docs.rs/wasmtime/latest/wasmtime/struct.Store.html).

### Capability-based sandboxing (Confidence: HIGH)
- Wasmtime's official security model: *"execute untrusted code in a safe manner inside of a sandbox."* **No ambient authority** — *"all interaction with the outside world is done through imports and exports. There is no raw access to system calls."* Everything is deny-by-default; the host must consciously grant each capability.
- WASI is **capability-based**: a component with an **empty `WasiCtx`** (no preopened dirs, no sockets, no HTTP handler) can touch nothing. Filesystem access is granted via **preopens** (`--dir` / programmatic preopen); network via explicit socket/`--tcplisten` grants; deny-by-default firewalling is expected on sockets. Defense-in-depth: guard regions, bounds-checked linear memory, stack-overflow checks, memory zeroing between instances, emerging CFI. Sources: [Wasmtime security.html](https://docs.wasmtime.dev/security.html), [WASI capability model](http://www.chikuwa.it/blog/2023/capability/), [systemshardening WASI roadmap](https://www.systemshardening.com/articles/wasm/wasip3-security-roadmap/).

### Resource limits (Confidence: HIGH)
- **Fuel** (`Config::consume_fuel` + `Store::fuel_async_yield_interval`): deterministic — wasm consumes a fixed amount of fuel per operation; futures yield `Poll::Pending` after a budget. Good for determinism/metering.
- **Epoch interruption** (`Config::epoch_interruption` + `Store::epoch_deadline_*` + `Engine::increment_epoch`): lower overhead than fuel (measured up to ~2–3x faster), non-deterministic, ideal for wall-clock timeouts / killing infinite loops.
- **Memory/table/instance limits**: the **`ResourceLimiter` trait** via `Store::limiter` caps memory, tables, and instance creation. Sources: [Config docs](https://docs.wasmtime.dev/api/wasmtime/struct.Config.html), [ResourceLimiter](https://docs.wasmtime.dev/api/wasmtime/trait.ResourceLimiter.html).

### Hot-reload (Confidence: MEDIUM)
- Wasmtime has no single "hot reload" API, but modules/components are compiled to an `Engine` and instantiated per-`Store`; you reload by compiling the new artifact and instantiating fresh, discarding old `Store`s. (Standard embedding pattern; no dedicated primary-source page found — flagged medium.)

### Component Model tooling (Confidence: HIGH on versions)
- **`wit-bindgen` 0.58.0 (2026-06-08)** — actively developed (67 versions), generates bindings for Rust/C/C++/C#/Go incl. inline proc-macros. [crates.io API](https://crates.io/api/v1/crates/wit-bindgen), [repo](https://github.com/bytecodealliance/wit-bindgen).
- **`cargo-component` 0.21.1 (2025-03-18)** — no release in ~15 months as of mid-2026 (flag: comparatively quiet vs. wit-bindgen's monthly cadence). It still targets `wasm32-wasip1` → adapts to WASIp2 components. Note: the modern Rust component workflow increasingly uses `wasm32-wasip2` target + `wit-bindgen` directly, which may explain cargo-component's slower cadence. Sources: [crates.io API](https://crates.io/api/v1/crates/cargo-component), [repo](https://github.com/bytecodealliance/cargo-component). Not deprecated. (Distinct from **wasm-bindgen**, a browser tool, whose rustwasm org is being archived/transferred — unrelated to Component Model tooling.)

---

## 2. Extism

### Versions & maintenance (Confidence: HIGH)
- **Host SDK (`extism` crate): 1.30.0, published 2026-06-04** (46 versions, 523k downloads, BSD-3-Clause). Actively maintained. [crates.io API](https://crates.io/api/v1/crates/extism), [docs.rs](https://docs.rs/extism/latest/extism/).
- **Rust PDK (`extism-pdk`): 1.4.1, published 2025-05-19** — the *crate* hasn't had a new release in ~14 months, though the repo saw commits into Feb 2026 (the "Feb 2026" signal is repo activity, not a crate release — flag this). [crates.io API](https://crates.io/api/v1/crates/extism-pdk).
- **Backing:** maintained by **Dylibso, Inc.** (© 2026 Dylibso on extism.org; commercial support offered). Reached **v1.0 in 2024**. Sources: [extism.org](https://extism.org/), [Dylibso: announcing Extism v1](https://dylibso.com/blog/announcing-extism-v1/).

### Architecture & sandbox (Confidence: HIGH)
- Extism is a **high-level abstraction that wraps lower-level engines — "Wasmtime, Wazero, V8, Spidermonkey, etc."** (the Rust host SDK is built on **wasmtime**). It ships a **ready-made ABI** so you exchange strings/bytes/JSON instead of hand-crafting an integer/float ABI. Sources: [Extism FAQ](https://extism.org/docs/questions/), [Dylibso: how Extism works](https://dylibso.com/blog/how-does-extism-work/).
- Security: *"fully sandboxes the execution of all plug-in code"*; the FAQ explicitly contrasts it as safe *"unlike a DLL or dlopen to a .dylib or .so file."* It **supports WASI as a superset** but deliberately withholds system-resource access by default (deny-by-default), inheriting wasmtime's sandbox. Sources: [extism.org](https://extism.org/), [Extism FAQ](https://extism.org/docs/questions/).

### Plugin (PDK) languages — the polyglot win (Confidence: HIGH)
- Write plugins in **Rust, Go, JavaScript/TypeScript (via QuickJS compiled to Wasm), C/C++, Haskell, AssemblyScript, Zig** (and Python/others via the XTP platform tooling). This is Extism's biggest ergonomic advantage for *user-supplied* plugins — non-Rust users can contribute. Sources: [Extism PDK concepts](https://extism.org/docs/concepts/pdk/), [js-pdk](https://github.com/extism/js-pdk), [assemblyscript-pdk](https://github.com/extism/assemblyscript-pdk).

### Host SDK languages (Confidence: HIGH)
- 15 host SDKs: Browser, C, C++, Elixir, Go, Haskell, Java, .NET, Node, OCaml, Perl, PHP, Python, Ruby, Rust, Zig. [extism.org](https://extism.org/).

### Host functions, capability grants, limits (Confidence: HIGH for existence, MEDIUM for exact field names)
- **Host functions**: Rust functions you write and pass into the plugin; callable from any guest language. A host function = name + optional input/output WASM-mapped types + optional `UserData` (must be `Send + Sync` for pooled/shared plugins). Sources: [Host Functions docs](https://extism.org/docs/concepts/host-functions/), [docs.rs/extism](https://docs.rs/extism/latest/extism/).
- **Capability grants via `Manifest`**: rich schema covering **allowed hosts (for outbound HTTP), allowed paths (filesystem mapping), timeouts, and memory limits** — deny-by-default; a plugin gets HTTP/FS only when the manifest lists it. (Exact field naming per SDK version should be confirmed against docs.rs `Manifest` — flagged medium.) Source: [docs.rs/extism Manifest](https://docs.rs/extism/latest/extism/).

### Hot reload & ergonomics vs raw wasmtime (Confidence: MEDIUM)
- Plugins are created from a `Manifest`; reload = construct a new `Plugin` from updated bytes (supports plugin pools/threads). No dedicated hot-reload page surfaced (flag medium).
- **Ergonomics**: Extism drastically cuts boilerplate vs raw wasmtime — you get a batteries-included ABI (strings/bytes/JSON), a uniform host-function model, manifest-based capability config, and polyglot PDKs, all on top of wasmtime's sandbox. Trade-off: you don't get the standards-track **Component Model/WIT typed interfaces**.
- **Component Model position**: Extism uses its **own custom ABI, not the Component Model** (as of mid-2026 no announced migration; the FAQ/homepage don't mention it). This is a deliberate design choice for simplicity/portability across engines. (Confidence MEDIUM — based on absence of any CM commitment in primary docs; I could not find an explicit "we will/won't adopt CM" statement, so flag as unverified intent.)

---

## 3. Native `.so` loading (libloading / abi_stable) — DANGEROUS for untrusted code (Confidence: HIGH)

- **`abi_stable` exists and is a real crate** but is **stale**: **0.11.3, last released 2023-10-12** (prior releases 2023-07, 2022-12). Maintainer rodrimati1992; ~3.4M downloads. It is *usable* but has had **no release in ~2.5+ years** — flag as low maintenance velocity. [crates.io API](https://crates.io/api/v1/crates/abi_stable), [repo](https://github.com/rodrimati1992/abi_stable_crates).
- **What it provides:** a stable Rust-to-Rust FFI ABI, **load-time type-layout checking** (recursively verifies type compatibility, allows semver-compatible changes), and an `AbortBomb` that **converts a cross-FFI panic into an abort** to avoid UB. It's *"just a wrapper over libloading."*
- **What it does NOT provide — the security-critical point:** *"abi_stable doesn't include a sandbox, so if the plugin developer was a malicious actor, they'd have full access to the computer the runtime is being executed on."* It is **not a security boundary**. A native `.so` runs with the **full privileges of your process** — arbitrary syscalls, filesystem, network, memory of your whole address space.
- **Additional native-plugin hazards:** panicking across an FFI boundary is **undefined behavior** unless every exported fn wraps in `catch_unwind` (or you use abi_stable's abort mechanism); Rust's default ABI/`repr(Rust)` layout is **unstable**, so version-mismatched `.so`s cause **silent memory corruption**. Sources: [NullDeref: Reducing Pain with abi_stable](https://nullderef.com/blog/plugin-abi-stable/), [NullDeref: Dynamic Loading](https://nullderef.com/blog/plugin-dynload/), [abi_stable docs.rs](https://docs.rs/abi_stable/latest/abi_stable/).
- **Verdict:** Fine for *first-party/trusted* plugins compiled by you. **Never** for untrusted user-supplied code. Even the abi_stable ecosystem author (Tremor use case) notes it's only acceptable because plugins come from trusted, manually-configured sources.
- Related/newer alternatives in the space: **`stabby`** (ZettaScaleLabs) offers a stable Rust ABI with compact sum-types — same security caveat applies (no sandbox). [stabby](https://github.com/ZettaScaleLabs/stabby).

---

## 4. Process isolation & MCP (Confidence: HIGH on tradeoffs, MEDIUM on JSON-lines specifics)

### The isolation spectrum (2026 best-practice framing)
- **In-process sandboxing of untrusted native code is perilous** — *"seccomp, ptrace, or custom sandboxes rarely survive production complexity."* The practical spectrum: **namespaces/seccomp** (fast, weak — namespaces are "visibility walls," **not** a security boundary; still allow host-kernel syscalls) → **gVisor** (user-space kernel, strong compatibility) → **microVMs** (hardware boundary, strongest) → **WASM** ("fastest with strong isolation of limited scope"). Sources: [UBOS sandbox isolation](https://ubos.tech/news/understanding-sandbox-isolation-namespaces-cgroups-seccomp-gvisor-and-webassembly/), [shayon.dev: Let's discuss sandbox isolation](https://www.shayon.dev/post/2026/52/lets-discuss-sandbox-isolation/).

### Process isolation over stdio (JSON-lines / MCP)
- **MCP** uses **JSON-RPC 2.0**; the default local transport is **stdio**, where the server runs as a **subprocess** of the host. Critically: with stdio, *"the MCP server runs as a subprocess... typically sharing the same user privileges and security context"* — i.e., **stdio alone is NOT isolation**; it inherits the parent's privileges. The Nov-2025 spec added **Streamable HTTP** (replacing SSE) for remote servers.
- MCP security guidance explicitly says: *"Untrusted MCP servers should run in containers or WASM sandboxes,"* assume zero-trust for third-party servers until verified. So process-per-plugin only becomes a *security* boundary when combined with **containers/namespaces + seccomp + cgroups** (or a microVM). Sources: [MCP architecture](https://modelcontextprotocol.io/docs/learn/architecture), [MCP security (COSAI/OASIS)](https://github.com/cosai-oasis/ws4-secure-design-agentic-systems/blob/main/model-context-protocol-security.md), [MCP 2026 guide](https://dev.to/x4nent/complete-guide-to-mcp-model-context-protocol-in-2026-architecture-implementation-and-4a11).

### When to choose which (synthesis)
**Prefer process isolation (separate OS process + seccomp/namespaces or microVM, JSON-lines/MCP over stdio) when:**
- Plugins need **native libraries / native performance** or existing binaries you can't recompile to WASM.
- You want an **OS/kernel-level** boundary (seccomp-BPF syscall filtering, namespaces, cgroups) or defense-in-depth beyond the WASM VM.
- Crash isolation: a plugin segfault kills only its process.
- You're already in an agent/tooling ecosystem where **MCP** is the interop standard.

**Prefer in-process WASM (wasmtime/Extism) when:**
- **Fast startup** (microseconds, no process/container spin-up) and **high call frequency** (no IPC serialization per call).
- **Determinism** (fuel metering) and **portability** across OS/arch.
- You want a **capability sandbox with no ambient authority** without shipping a container runtime inside your process. The 2026 consensus: *"the WebAssembly Component Model plus WASI Preview 2 has matured into a credible, practical answer... a polyglot, portable, capability-secure runtime... without requiring an entire container runtime inside your process."* Source: [Debugg.ai: WASM Component Model 2025](https://debugg.ai/resources/wasm-component-model-2025-polyglot-plugins-secure-extensibility).

**Complementary, not exclusive:** a common 2026 pattern is WASM for the hot path + process/microVM isolation for plugins needing native access, sometimes both behind MCP.

---

## Recommendation for your use case (untrusted, user-supplied feature modules)

- **Default choice: Extism (`extism` 1.30.0 on wasmtime).** Best ergonomics-to-security ratio: real wasmtime sandbox, deny-by-default manifest (allowed_hosts/allowed_paths/memory/timeout), host functions to expose exactly the APIs you want, and **polyglot PDKs** so users write plugins in Rust/Go/JS/Python/Zig/etc. Trade-off: custom ABI, not Component Model; PDK crate cadence is slowish.
- **If you want standards-track & typed interfaces / async streaming:** raw **wasmtime 46 + Component Model + WASI 0.2 (stable)**, optionally WASI 0.3 async. More boilerplate (WIT + wit-bindgen 0.58), maximum control over fuel/epoch/memory limits and capability grants.
- **Avoid** native `.so` (libloading/abi_stable/stabby) for untrusted code — **no sandbox, full host compromise**, plus panic-across-FFI UB and ABI-mismatch corruption.
- **Add process/OS isolation** (separate process + seccomp/namespaces or microVM, JSON-lines or MCP over stdio) only for plugins that genuinely need native libraries, or as a second defense layer — remembering stdio/MCP by itself grants the child your privileges.

### Confidence & flags
- **HIGH:** all version numbers/dates (crates.io API + GitHub), WASI 0.2 stable / WASI 0.3 launched 2026-06-11, wasmtime security model, abi_stable's "no sandbox / malicious = full host access," native-plugin hazards, isolation-spectrum tradeoffs.
- **MEDIUM:** Extism exact manifest field names per SDK version; Extism/wasmtime hot-reload mechanics (no dedicated primary page); Extism's stance on adopting the Component Model (inferred from absence, not an explicit statement).
- **Flags:** `extism-pdk` crate (1.4.1) and `cargo-component` (0.21.1) both show noticeably slower release cadence than the rest of the ecosystem — verify they still fit your toolchain before committing. `abi_stable` last released 2023 (low velocity).