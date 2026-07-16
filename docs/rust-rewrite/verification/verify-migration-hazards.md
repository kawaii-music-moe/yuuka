Now I have a clear picture of the actual architecture. Key facts confirmed:
- SQLite WAL, single writer = Node (`better-sqlite3`), no `busy_timeout` set on Node side
- Existing Rust daemon (`yuuka-synapse`) already reads the same DB read-only with `busy_timeout(3000)` — a working precedent
- Session token is opaque CSPRNG, stored hashed in shared Redis (not a signed cookie) → sharing via Redis, not via a signing key
- Cookie: `__Host-yuuka-session`, `SameSite=Lax`, `HttpOnly`, `Secure`, `Path=/`
- WebSocket `/ws/chat` via `noServer` + `handleUpgrade`, auth via cookie→`getSession`
- nginx uses `proxy_pass` with per-location upstreams

Now let me research the primary sources for each hazard. This is the critical part — SQLite concurrent access semantics. Let me launch parallel research agents and do targeted web searches myself.