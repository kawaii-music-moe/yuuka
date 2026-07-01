I have comprehensive primary-source coverage across all three topics. The Thoughtworks article gave useful quotes on data synchronization and observability. I have everything needed. Let me compile the final writeup.

---

# Node→Rust Strangler-Fig 移行: 一次情報に基づく技術調査レポート

以下、3トピックすべてを **結論 / 根拠(引用+URL) / 推奨手順 / 落とし穴** の構成でまとめる。引用は原文(英語)を維持し、すべての主張に URL を付与している。今日は 2026-07 時点。

---

## TOPIC A — nginx リバースプロキシで URL パスにより旧 Node / 新 Rust バックエンドを分岐

### A-1. `location` マッチング優先順位と `proxy_pass` の URI 書き換え

**結論**
nginx の `location` は「① `=` 完全一致 → ② 最長プレフィックス一致(`^~` 付きならその時点で正規表現をスキップ)→ ③ 正規表現(設定ファイル出現順、最初のヒットで確定)→ ④ 正規表現に一致しなければ記憶したプレフィックス location を採用」という順序で解決される。パス分岐は `location /api/v2/ { proxy_pass http://rust_backend; }` のようにプレフィックス location + upstream で行うのが正典。`proxy_pass` に URI を付けるか付けないかで、リクエスト URI の書き換え挙動が根本的に変わる。

**根拠(引用+URL)**

マッチングアルゴリズム(nginx.org 公式):
> "To find location matching a given request, nginx first checks locations defined using the prefix strings (prefix locations). Among them, the location with the longest matching prefix is selected and remembered. Then regular expressions are checked, in the order of their appearance in the configuration file. The search of regular expressions terminates on the first match, and the corresponding configuration is used. If no match with a regular expression is found then the configuration of the prefix location remembered earlier is used."
> "If the longest matching prefix location has the `^~` modifier then regular expressions are not checked."
> "Using the `=` modifier it is possible to define an exact match of URI and location. If an exact match is found, the search terminates."
— https://nginx.org/en/docs/http/ngx_http_core_module.html#location

`proxy_pass` の URI 書き換え(nginx.org 公式、原文):
- **URI を付けた場合(location にマッチした部分が置換される)**:
> "If the `proxy_pass` directive is specified with a URI, then when a request is passed to the server, the part of a normalized request URI matching the location is replaced by a URI specified in the directive"
> ```
> location /name/ { proxy_pass http://127.0.0.1/remote/; }
> ```
- **URI を付けない場合(元 URI がそのまま渡る)**:
> "If `proxy_pass` is specified without a URI, the request URI is passed to the server in the same form as sent by a client when the original request is processed, or the full normalized request URI is passed when processing the changed URI"
> ```
> location /some/path/ { proxy_pass http://127.0.0.1; }
> ```
- **URI が決定できない=URI を付けてはいけないケース(正規表現 location・named location)**:
> "When location is specified using a regular expression, and also inside named locations. In these cases, `proxy_pass` should be specified without a URI."
- **rewrite … break 後は指定 URI が無視される**:
> "In this case, the URI specified in the directive is ignored and the full changed request URI is passed to the server."
- **変数を使った場合は URI がそのまま渡る**:
> "When variables are used in `proxy_pass` … In this case, if URI is specified in the directive, it is passed to the server as is, replacing the original request URI."

— https://nginx.org/en/docs/http/ngx_http_proxy_module.html#proxy_pass

トレーリングスラッシュに関する 301 挙動:
> "If a location is defined by a prefix string that ends with the slash character, and requests are processed by one of proxy_pass … then … In response to a request with URI equal to this string, but without the trailing slash, a permanent redirect with the code 301 will be returned to the requested URI with the slash appended."
— https://nginx.org/en/docs/http/ngx_http_core_module.html#location

**推奨手順(パス分岐の正典パターン)**
```nginx
upstream node_backend { server 127.0.0.1:3000; }
upstream rust_backend { server 127.0.0.1:8080; }

server {
    # 旧: すべて Node へ(記憶される最長プレフィックス)
    location / {
        proxy_pass http://node_backend;   # URI なし → パスをそのまま Node へ
    }
    # 移行済みスライスだけ Rust へ切り出す
    location /api/v2/ {
        proxy_pass http://rust_backend;   # URI なし → /api/v2/... がそのまま Rust へ
    }
}
```
- 移行スライスごとに `location` を1本追加する(strangler-fig のカットオーバー単位)。
- **`proxy_pass` に URI を付けない**ことを推奨。付けない限り nginx はパスを書き換えず素通しするため、両バックエンドが同一パス空間を共有でき、パス取り違えのバグを避けられる(上記引用 (2))。

**落とし穴**
- `proxy_pass http://rust_backend/;`(末尾スラッシュ=URI 付き)にすると `/api/v2/foo` が `/foo` へ書き換わる(引用 (1))。付ける/付けないでルーティングが静かに壊れる。
- 正規表現 location(`location ~ ^/api/`)で `proxy_pass http://rust_backend/xxx;` のように URI を付けると設定エラー/意図しない挙動になる。正規表現 location では URI を付けてはいけない(引用 (3))。
- プレフィックス最長一致は「定義順」ではなく「長さ」で決まる。`/api/` と `/api/v2/` を両方定義すると `/api/v2/...` は必ず後者に行く(意図通りだが、順序で制御していると誤解しやすい)。

---

### A-2. WebSocket プロキシに必須のディレクティブ

**結論**
`Upgrade` / `Connection` は **hop-by-hop ヘッダ**であり、リバースプロキシではデフォルトで下流へ渡らない。したがって WebSocket を通すには `proxy_set_header Upgrade $http_upgrade;` と `Connection` ヘッダを明示設定する必要がある。`proxy_http_version 1.1;` は **nginx 1.29.7 より前**では必須だったが、それ以降は不要(自動化された)。無通信 60 秒でコネクションが切れるため長時間 WS には `proxy_read_timeout` を延長する。

**根拠(引用+URL、すべて nginx.org 公式)**

hop-by-hop の理由:
> "There is one subtlety however: since the 'Upgrade' is a hop-by-hop header, it is not passed from a client to proxied server. … special processing on a proxy server is required."
> "As noted above, hop-by-hop headers including 'Upgrade' and 'Connection' are not passed from a client to proxied server, therefore in order for the proxied server to know about the client's intention to switch a protocol to WebSocket, these headers have to be passed explicitly:"

基本設定(原文):
```nginx
location /chat/ {
    proxy_pass http://backend;
    # proxy_http_version 1.1; # before version 1.29.7
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
}
```

`map` を用いた推奨形(条件付き Connection ヘッダ):
```nginx
http {
    map $http_upgrade $connection_upgrade {
        default upgrade;
        ''      close;
    }
    server {
        location /chat/ {
            proxy_pass http://backend;
            # proxy_http_version 1.1; # before version 1.29.7
            proxy_set_header Upgrade $http_upgrade;
            proxy_set_header Connection $connection_upgrade;
        }
    }
}
```

タイムアウト(原文):
> "By default, the connection will be closed if the proxied server does not transmit any data within 60 seconds. This timeout can be increased with the proxy_read_timeout directive. Alternatively, the proxied server can be configured to periodically send WebSocket ping frames to reset the timeout and check if the connection is still alive."

トンネルの前提(101 応答):
> "Since version 1.3.13, nginx implements special mode of operation that allows setting up a tunnel between a client and proxied server if the proxied server returned a response with the code 101 (Switching Protocols), and the client asked for a protocol switch via the 'Upgrade' header in a request."

— https://nginx.org/en/docs/http/websocket.html

**推奨手順**
- `http` ブロックに `map $http_upgrade $connection_upgrade { default upgrade; '' close; }` を1つ置く。
- WS を扱う `location` で `Upgrade`/`Connection` を上記の通り設定。
- 長時間接続用に `proxy_read_timeout`(および必要なら `proxy_send_timeout`)を延長するか、Rust 側で ping フレームを定期送出。
- nginx が 1.29.7 以上なら `proxy_http_version 1.1;` は省略可。それ未満のバージョンを使うなら必ず記述する。

**落とし穴**
- `Upgrade`/`Connection` を書き忘れると WS ハンドシェイクが 200/400 で失敗する(hop-by-hop のため素通ししない)。
- デフォルト 60 秒タイムアウトでアイドル WS が切れる。移行対象が WS を使うなら旧 Node・新 Rust の**両方**の location に同じ WS 設定を入れること。
- `Connection "upgrade"` をベタ書きすると非 WS リクエストで keep-alive が壊れる場合があるため、`map` の `$connection_upgrade` 形が安全。

---

### A-3. `proxy_pass` に変数を使う場合の `resolver` とパス処理

**結論**
`proxy_pass` の値に変数を含めると、アドレスがドメイン名の場合まず upstream 群を検索し、なければ `resolver` で解決される。さらに変数を使うと nginx は自動的なパス置換をやめるため、`$request_uri` を明示的に付けてパスを渡す必要がある。

**根拠(引用+URL、nginx.org 公式)**
> "Parameter value can contain variables. In this case, if an address is specified as a domain name, the name is searched among the described server groups, and, if not found, is determined using a resolver."
> "When variables are used in `proxy_pass`: `proxy_pass http://127.0.0.1$request_uri;` … In this case, if URI is specified in the directive, it is passed to the server as is, replacing the original request URI."
— https://nginx.org/en/docs/http/ngx_http_proxy_module.html#proxy_pass

**推奨手順**
- 可能なら**変数を使わず** `upstream` ブロック + 静的 `proxy_pass` を使う(移行では upstream 定義が最も予測可能)。
- どうしても動的アップストリーム(例: Docker のサービス名を DNS 解決)が必要なら `resolver 127.0.0.11 valid=10s;` 等を設定し、`proxy_pass http://$rust_host$request_uri;` のようにパスを明示的に付与する。

**落とし穴**
- 変数を使った瞬間、nginx は「location にマッチした部分を置換する」自動書き換えを行わなくなる。`$request_uri` を付け忘れるとパスが欠落する。
- `resolver` 無しで変数ドメインを使うと起動時ではなくリクエスト時にエラーになる。
- 変数入り `proxy_pass` は起動時に upstream の存在チェックが効かないため、typo が実行時まで表面化しない。

---

## TOPIC B — 2 バックエンド間でのセッションクッキー共有

前提(コードベース既知情報、再検証不要): Cookie 名 `__Host-yuuka-session`、属性 `Path=/; HttpOnly; Secure; SameSite=Lax`。トークンは **不透明な CSPRNG ランダム文字列**(署名/JWT ではない)。サーバは `sha256(token)` を Redis キー `session:{sha256(token)}` として保存(値がセッションペイロード)、Redis ダウン時はインメモリ Map にフォールバック。検証=トークンをハッシュ化して Redis 参照。

### B-1. `__Host-` プレフィックスが要求する制約(同一オリジン前提)

**結論**
`__Host-` プレフィックス付き Cookie は、ブラウザが受理する前に **(1) Secure 属性が付いている (2) HTTPS(secure)オリジンから送られた (3) Domain 属性が無い(host-only) (4) Path=/** の**すべて**を満たすことを強制する。これらのいずれかが欠けるとブラウザは Cookie を**丸ごと破棄**する。Domain 属性が禁止=サブドメイン共有不可であるため、旧 Node と新 Rust は**同一プロキシ配下の同一オリジンで**動く必要がある(これが TOPIC A のパス分岐リバースプロキシを必須にする理由)。

**根拠(引用+URL)**

MDN(原文):
> "`__Host-`: Cookies with names starting with `__Host-` must be set with the `Secure` attribute by a secure page (HTTPS). In addition, they must not have a `Domain` attribute specified, and the `Path` attribute must be set to `/`. This guarantees that such cookies are only sent to the host that set them, and not to any other host on the domain."
— https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Set-Cookie

RFC 6265bis(規範的 MUST、httpwg 公式ドラフト):
> "If the cookie-name begins with a case-insensitive match for the string '__Host-', abort this algorithm and ignore the cookie entirely unless the cookie meets all the following criteria: 1. The cookie's secure-only-flag is true. 2. The cookie's host-only-flag is true. 3. The cookie-attribute-list contains an attribute with an attribute-name of 'Path', and the cookie's path is `/`."
> "If the request-uri does not denote a 'secure' connection … and the cookie's secure-only-flag is true, then abort these steps and ignore the cookie entirely."
— https://httpwg.org/http-extensions/draft-ietf-httpbis-rfc6265bis.html
(ドラフト一覧: https://datatracker.ietf.org/doc/draft-ietf-httpbis-rfc6265bis/)

`host-only-flag=true` は「Set-Cookie に Domain 属性が無かった」ことを意味する(=同一ホストのみに送信)。

**推奨手順**
- 旧・新の両バックエンドを**単一の nginx オリジン**(同一ホスト名・HTTPS)の背後に置き、TOPIC A のパス分岐で振り分ける。これで両者が**同一ホスト**として同じ `__Host-` Cookie を送受信できる。
- Set-Cookie は必ず `Secure; Path=/; Domain 属性なし; HTTPS 経由`。旧・新どちらが Set-Cookie を発行しても同一属性で出す。

**落とし穴**
- 別オリジン/別サブドメイン(例: `api.example.com` と `example.com`)に分けると `__Host-` は Domain 指定不可なので**そもそも共有できない**。移行中は必ず同一オリジン+パス分岐。
- 開発環境が HTTP だと `__Host-` Cookie がブラウザに拒否される(Secure 必須)。ローカルでも HTTPS 化するか環境ごとに Cookie 名を切り替える。

---

### B-2. 不透明トークン+共有ストア方式で 2 バックエンドが一致させるべきもの(署名鍵は不要)

**結論**
不透明トークン(=リファレンストークン)を共有 Redis で参照する方式では、両バックエンドで**トークンのハッシュアルゴリズム(sha256)・Redis キー書式(`session:{sha256(token)}`)・値のシリアライズ形式・TTL セマンティクス**を**完全に一致**させれば同一 Cookie を検証できる。トークン自体には意味が無くストア参照するだけなので、**共有署名鍵/HMAC シークレットは不要**。これは JWT/署名 Cookie 方式との決定的な差異で、JWT 方式なら両者で検証鍵の共有が必須になる。

**根拠(引用+URL)**

OWASP: セッション ID は不透明でありサーバ側にロジックを持つ:
> "The session ID content (or value) must be meaningless to prevent information disclosure attacks."
> "The meaning and business or application logic associated with the session ID must be stored on the server side, and specifically, in session objects or in a session management database or repository."
> "Session identifiers must have at least `64 bits` of entropy … A strong CSPRNG … must be used to generate session IDs."
— https://cheatsheetseries.owasp.org/cheatsheets/Session_Management_Cheat_Sheet.html

不透明(リファレンス)トークン vs JWT(自己完結)トークンの検証モデル差:
> "Opaque tokens act as reference identifiers that require server-side introspection … An opaque token is a random, high-entropy string with no readable structure. The claims live server-side, in a token store the authorization server controls."
> "A JWT is meant to be stateless and self-contained—it has all the information the server needs except the signing keys, so the server doesn't need to store this information server-side."
— https://zitadel.com/blog/jwt-vs-opaque-tokens

> "Because the token cannot be parsed locally, any API layer receiving it must execute a network call to the authorization server to validate its integrity … "
> "JWTs enable local, stateless evaluation through cryptographic signature verification, eliminating the need for backend database lookups."
— https://nordicapis.com/jwt-vs-opaque-tokens-choosing-the-right-token-for-api-security/

→ この対比が「不透明トークンはストア参照ゆえ**共有鍵不要**、JWT/署名 Cookie は**共有検証鍵が必須**」を裏付ける。本件は前者。

共有 Redis セッションストアで一致させるべき項目(キー書式・シリアライズ・TTL):
> "Each session is typically stored as a Redis hash with a key like session:{session_id}. When multiple applications share the same Redis instance, you may need unique prefixes … to avoid key collisions."
> "When you have multiple applications that use the same Redis instance but have different versions of the same class, it might be problematic … if different services serialize session data differently … data created by one service may not be readable by another."
> "The TTL is reset every time a user interacts with the server to keep active sessions alive."
— https://redis.io/docs/latest/develop/use-cases/session-store/nodejs/(Redis 公式ユースケース)
— https://oneuptime.com/blog/post/2026-01-21-redis-shared-state-microservices/view

**推奨手順**
1. **ハッシュ**: 両バックエンドで `sha256(token)` を同一エンコーディング(hex か base64 のどちらか、大小文字含め)で算出。
2. **キー書式**: `session:{sha256(token)}` を一字一句同一に。プレフィックスをサービス別に変えない(共有が目的なので**同一キースペース**にする)。
3. **値のシリアライズ**: JSON のフィールド名・型・日時表現を共通化。Rust(serde)側と Node 側で往復可能なスキーマにし、契約テストで両方向のデシリアライズを検証。
4. **TTL セマンティクス**: 期限秒数・スライディング更新(アクセス毎に `EXPIRE` リセット)するか否かを揃える。片方だけスライディングだと片側で早期失効する。
5. **署名鍵は不要**だが、Redis への接続(認証/TLS)は両者に必要。

**落とし穴**
- シリアライズ形式のズレ(camelCase vs snake_case、Unix 秒 vs ISO8601)で「片方が書いたセッションを他方が読めない」障害が起きる。契約テスト必須。
- TTL 更新ポリシーの不一致でセッションが片側だけ「生きている」状態になる。
- キープレフィックスをサービス別に分けると共有できない。ここでは意図的に**同一キースペース**にする。

---

### B-3. Redis 共有ストア+インメモリフォールバックの gotcha

**結論**
インメモリフォールバックは**プロセスローカル**であり、他バックエンドから見えない。あるバックエンドが Redis に到達できずローカル Map にセッションを書くと、そのセッションは**もう一方のバックエンドには不可視**になり、リクエストが別バックエンドにルーティングされた瞬間にセッションが「消える」。フォールバックは可用性のための一時退避であって、共有セッションの一貫性は保証しない。

**根拠(引用+URL)**
共有 Redis が「サーバをまたいだログイン維持」を可能にするのは、状態が**単一の共有ストア**にあるから:
> "Using Redis solves the multi-server problem where a user logged in on server A would get logged out when their next request hits server B."
> (ローカルフォールバックについて)"if one service falls back to in-memory sessions, other services sharing Redis wouldn't see those fallback sessions."
— https://oneuptime.com/blog/post/2026-01-21-redis-shared-state-microservices/view
— https://redis.io/docs/latest/develop/use-cases/session-store/nodejs/

**推奨手順**
- インメモリフォールバックは「Redis 一時断でも**新規/既存セッションを完全に落とさない**ための劣化運転」と位置づけ、Redis 復帰後は Redis を single source of truth に戻す。
- フォールバック発動を**メトリクス/アラート**化(Redis 断は SPOF リスクなので即検知)。
- 可能なら Redis を HA 化(Sentinel/Cluster/マネージド)し、フォールバック依存を最小化。
- フォールバック中は**アフィニティ(sticky routing)を持たせる**か、そもそもフォールバック中は片系に固定してセッションの見え方の分裂を防ぐ。

**落とし穴**
- フォールバック中に nginx が旧↔新をパスで分岐すると、同一ユーザーの連続リクエストが別プロセスに渡り、ローカル Map のセッションが見えず**断続的ログアウト**が発生する(再現困難なフラッピング障害)。
- フォールバック Map の TTL/エビクションが Redis と異なると、復帰時にセッション状態が不整合になる。
- 「Redis が落ちても動いているように見える」ため障害が隠蔽されやすい。フォールバック発動自体を必ず可観測にする。

---

## TOPIC C — Strangler-Fig のアンチパターンとフェーズ完了/ロールバック設計

### C-1. Martin Fowler の正典定義とカットオーバーに関する助言

**結論**
Strangler Fig は「レガシーの周囲に新機能を少しずつ足し、段階的に置き換えていく**漸進的近代化**」であり、ビッグバン書き換えのリスク回避が本質。頻繁なカットオーバーと、新旧共存のための**過渡的アーキテクチャ(transitional architecture)**の受容が要点。

**根拠(引用+URL)**
> "These are vines that germinate in a nook of a tree. As it grows, it draws nutrients from the host tree until it reaches the ground to grow roots …"
> "a gradual process of modernization. Like the fig, it begins with small additions, often new features, that are built on top of, yet separate to the legacy code base."
> "Replacing a serious IT system takes a long time, and the users can't wait for new features. Replacements seem easy to specify, but often it's hard to figure out the details of existing behavior."
> (過渡的アーキテクチャについて)"people often balk at the necessity of building transitional architecture … code that will go away once the modernization is complete." / "the reduced risk and earlier value from the gradual approach outweigh its costs."
— https://martinfowler.com/bliki/StranglerFigApplication.html

---

### C-2. 一般的アンチパターン(権威ソース)

**結論**
主要アンチパターン: (1) **ロールバック経路が無い** (2) **strangler の中でビッグバン**(スライスが大きすぎ) (3) **移行中の共有 DB による密結合**(=論理的には 1 システムのまま) (4) **ファサード/プロキシの恒久化** (5) **可観測性の欠如**。加えてプロキシ層が **SPOF/ボトルネック**になる点。

**根拠(引用+URL)**

プロキシ層が SPOF/ボトルネックになる(Microsoft・AWS 両公式):
> "Make sure that the façade doesn't become a single point of failure or a performance bottleneck."
> "Make sure that the façade keeps up with the migration."
— https://learn.microsoft.com/en-us/azure/architecture/patterns/strangler-fig
> "Proxy layer failure: During migration, a proxy layer intercepts the requests … However, this proxy layer can become a single point of failure or a performance bottleneck."
— https://docs.aws.amazon.com/prescriptive-guidance/latest/cloud-design-patterns/strangler-fig.html

ファサードは過渡的で恒久化させない(Microsoft 公式):
> "After the migration is complete, you typically remove the strangler fig façade. Alternatively, you can maintain the façade as an adapter for legacy clients … Conceptualize this as transitional architecture, and balance this architecture's risk mitigation benefits against its temporary infrastructural costs."
— https://learn.microsoft.com/en-us/azure/architecture/patterns/strangler-fig

共有 DB / データ同期の密結合はタクティカル扱い(AWS 公式):
> "Data consistency: The microservices own their data store, and the monolithic application can also potentially use this data … this can cause data redundancy and eventual consistency between two data stores, so we recommend that you treat it as a tactical solution until you can establish a long-term solution …"
> "Consider how to handle services and data stores that both the new system and the legacy system might use. Make sure that both systems can access these resources at the same time."(Microsoft)
— https://docs.aws.amazon.com/prescriptive-guidance/latest/cloud-design-patterns/strangler-fig.html
— https://learn.microsoft.com/en-us/azure/architecture/patterns/strangler-fig

ビッグバン回避(AWS 公式):
> "A big bang migration, where the monolith is migrated in a single operation, introduces transformation risk and business disruption."
— https://docs.aws.amazon.com/prescriptive-guidance/latest/cloud-design-patterns/strangler-fig.html

小規模アプリでは strangler 自体が過剰(Microsoft・AWS):
> "You migrate a small system and replacing the whole system is simple."(適用不可条件)
— https://learn.microsoft.com/en-us/azure/architecture/patterns/strangler-fig

データ同期と可観測性(Thoughtworks):
> "Running both old and new systems side by side, a characteristic of the Strangler Fig approach, necessitates data synchronization, which presents challenges such as ensuring data consistency and integrity, managing performance impacts…"
> "…having a world-class observability apparatus comes into play."
— https://www.thoughtworks.com/insights/articles/embracing-strangler-fig-pattern-legacy-modernization-part-three

**ACL(腐敗防止層)**は新旧相互呼び出しの分離に使い、移行完了後に撤去する(AWS 公式):
> "The ACL must be decommissioned after all dependent services have been migrated into the microservices architecture."
— https://docs.aws.amazon.com/prescriptive-guidance/latest/cloud-design-patterns/strangler-fig.html

---

### C-3. Feature flag / カナリア / シャドウ(ミラー)トラフィック

**結論**
- **カナリア**: 新版を少数のユーザー/サーバに先行展開し、問題があれば**旧版へ即再ルーティング**して戻す。バックエンド移行のスライス投入に最適。
- **シャドウ/ミラー(dark launch)**: 実トラフィックを複製して新バックエンドへ送るが応答はユーザーに返さない。nginx の `ngx_http_mirror_module`(`mirror` ディレクティブ)で実現。**重大な注意**: ミラーしたリクエストが**共有 DB に書き込む/副作用を持つ**新バックエンドに届くと**二重書き込み・二重課金・重複メール**等を起こす。ゆえにシャドウは**読み取り専用/冪等なエンドポイント**、または**分離したストア**に限って安全。

**根拠(引用+URL)**

カナリア(Martin Fowler):
> "a technique to reduce the risk of introducing a new software version in production by slowly rolling out the change to a small subset of users before rolling it out to the entire infrastructure"
> "if you find any problems with the new version, the rollback strategy is simply to reroute users back to the old version until you have fixed the problem."
— https://martinfowler.com/bliki/CanaryRelease.html

nginx mirror(nginx.org 公式):
> "The `ngx_http_mirror_module` module (1.13.4) implements mirroring of an original request by creating background mirror subrequests. Responses to mirror subrequests are ignored."
> ```
> location / { mirror /mirror; proxy_pass http://backend; }
> location = /mirror { internal; proxy_pass http://test_backend$request_uri; }
> ```
— https://nginx.org/en/docs/http/ngx_http_mirror_module.html

シャドウ/ミラーの副作用の注意(定義: Microsoft Engineering Playbook):
> "we're replicating the same traffic with V-Current environment and directing same traffic to V-Next environment, the only difference is we don't return any response from V-Next environment to users"
— https://microsoft.github.io/code-with-engineering-playbook/automated-testing/shadow-testing/

二重書き込み・副作用への具体的警告(業界ソース):
> "Shadow traffic should be used with caution or sampled to prevent unexpected issues, and capturing payment twice could lead to double charge of the customer."
> "Do not allow the shadow service to write to production databases or interact with live downstream systems. Instead, point it to staging versions of dependent services or use dummy endpoints. This prevents unintended side effects (such as duplicate transactions) …"
> "Mirrored services might inadvertently trigger actions such as sending emails or pushing notifications."
> "special care is needed to ensure only stateless requests are mirrored, or that the shadow service handles state independently to avoid conflicts with shared resources like databases."
— https://medium.com/doctolib/shadow-traffic-a-guide-to-reduce-risk-of-a-service-deployment-c67fd4ca3528
— https://infoq.com/articles/microservices-traffic-mirroring-istio-vpc/

Feature flag / 安全な展開(Microsoft WAF が strangler と整合するとする根拠):
> "OE:11 Safe deployment practices" / "This pattern's incremental approach can help mitigate risks during a component transition compared to making large systemic changes all at once."
— https://learn.microsoft.com/en-us/azure/architecture/patterns/strangler-fig

**推奨手順(移行スライス投入フロー)**
1. **シャドウ**でまず検証: 新 Rust エンドポイントを nginx `mirror` で受け、応答は破棄。ただし**読み取り専用/冪等**なエンドポイントに限定するか、Rust 側を**書き込み無効(dry-run)/分離 DB**にする。
2. **カナリア**: 問題なければ実トラフィックの数%を Rust へ振る(nginx split_clients / upstream weight / feature flag)。監視。
3. **段階的引き上げ**: メトリクス健全なら比率を上げる。異常時は旧 Node へ即戻す。
4. **完全カットオーバー**: 100% を Rust に。旧経路は当面**温存(warm)**。

**落とし穴(シャドウ特有)**
- ミラー先の新バックエンドが**共有 Redis セッションや共有 DB に書く**と、二重書き込み・セッション汚染が起きる。TOPIC B の共有 Redis はまさに副作用対象なので、シャドウでは Rust の書き込みを無効化するか別 Redis を使う。
- `mirror` は本流のレイテンシに影響しうる(サブリクエスト生成)。負荷を見て `mirror_request_body off` やサンプリングを検討。

---

### C-4. フェーズ完了基準とロールバック設計

**結論**
スライスごとに「done」を明示的に定義し、**旧経路を温存(warm)**して**即時ロールバック**可能にする。レガシーオブジェクト(旧 DB テーブル・旧コード経路)の削除は**各ドメインの最終ステップ**として意図的に行い、削除するまではロールバック可能である、というのが公式ガイダンスの立場。

**根拠(引用+URL、Microsoft 公式)**
> "You can roll back to the monolithic database during phase 2 and at the start of phase 3, when the domain tables and synchronization processes still exist in the monolithic database. To roll back … after you remove the domain tables … you must restore those objects and replay data changes. However, this process significantly increases effort and risk. Treat the removal of legacy objects as a deliberate final step for each domain. Remove legacy objects only after the new system is validated."
> "Validate consistency between both databases before cutover."
— https://learn.microsoft.com/en-us/azure/architecture/patterns/strangler-fig

カナリアのロールバック(即再ルーティング):
> "the rollback strategy is simply to reroute users back to the old version until you have fixed the problem."
— https://martinfowler.com/bliki/CanaryRelease.html

**推奨手順(スライスごとの完了/ロールバック基準)**
- **完了基準(per slice)**: 新 Rust エンドポイントが (a) 機能パリティ(契約テスト合格) (b) カナリアで所定期間エラー率/レイテンシが SLO 内 (c) セッション・データの整合性検証済み——を満たしたら「done」。
- **ロールバック warm 維持**: 旧 Node の該当 `location` を残し、nginx の1行変更(weight/flag)で即座に旧経路へ戻せる状態を保つ。レガシーコード/テーブルは**検証完了後の最終ステップ**まで削除しない。
- **可観測性**: 旧/新それぞれのエラー率・レイテンシ・セッション不整合を計測(Thoughtworks の "world-class observability" 引用に整合)。

**落とし穴**
- レガシー削除を早まると、ロールバックは「オブジェクト復元+データ再生」となり工数・リスクが激増する(上記 Microsoft 引用)。削除は**必ず最後**。
- ファサード(nginx)が恒久化して撤去計画を失うと、strangler が「2 デプロイ単位・1 論理システム」のまま固着する(C-2 の共有 DB 密結合と同根)。撤去/DB 分離のタイムラインを明示的に持つこと。
- カットオーバー前のデータ整合性検証("Validate consistency … before cutover")を省くと、切替後に不整合が顕在化する。

---

## 参照 URL 一覧(一次情報中心)

nginx 公式:
- location マッチング: https://nginx.org/en/docs/http/ngx_http_core_module.html#location
- proxy_pass / resolver: https://nginx.org/en/docs/http/ngx_http_proxy_module.html#proxy_pass
- WebSocket プロキシ: https://nginx.org/en/docs/http/websocket.html
- mirror モジュール: https://nginx.org/en/docs/http/ngx_http_mirror_module.html

Cookie / セッション:
- MDN Set-Cookie(`__Host-`/`__Secure-`): https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Set-Cookie
- MDN Secure cookie configuration: https://developer.mozilla.org/en-US/docs/Web/Security/Practical_implementation_guides/Cookies
- RFC 6265bis(httpwg): https://httpwg.org/http-extensions/draft-ietf-httpbis-rfc6265bis.html / https://datatracker.ietf.org/doc/draft-ietf-httpbis-rfc6265bis/
- OWASP Session Management Cheat Sheet: https://cheatsheetseries.owasp.org/cheatsheets/Session_Management_Cheat_Sheet.html
- 不透明 vs JWT(ZITADEL): https://zitadel.com/blog/jwt-vs-opaque-tokens
- 不透明 vs JWT(Nordic APIs): https://nordicapis.com/jwt-vs-opaque-tokens-choosing-the-right-token-for-api-security/
- Redis セッションストア公式: https://redis.io/docs/latest/develop/use-cases/session-store/nodejs/
- 共有 Redis 状態(OneUptime): https://oneuptime.com/blog/post/2026-01-21-redis-shared-state-microservices/view

Strangler Fig / デプロイ戦略:
- Martin Fowler StranglerFigApplication: https://martinfowler.com/bliki/StranglerFigApplication.html
- Martin Fowler CanaryRelease: https://martinfowler.com/bliki/CanaryRelease.html
- Microsoft Learn Strangler Fig: https://learn.microsoft.com/en-us/azure/architecture/patterns/strangler-fig
- AWS Prescriptive Guidance Strangler Fig: https://docs.aws.amazon.com/prescriptive-guidance/latest/cloud-design-patterns/strangler-fig.html
- Thoughtworks Strangler Fig (Part 3): https://www.thoughtworks.com/insights/articles/embracing-strangler-fig-pattern-legacy-modernization-part-three
- Microsoft Engineering Playbook Shadow Testing: https://microsoft.github.io/code-with-engineering-playbook/automated-testing/shadow-testing/
- Doctolib Shadow Traffic ガイド: https://medium.com/doctolib/shadow-traffic-a-guide-to-reduce-risk-of-a-service-deployment-c67fd4ca3528
- InfoQ Traffic Mirroring: https://infoq.com/articles/microservices-traffic-mirroring-istio-vpc/

---

### 検証レポート向け要点(3行サマリ)
1. **TOPIC A**: 分岐は `upstream`+プレフィックス `location`+`proxy_pass`(URI なし推奨)。WS は `Upgrade`/`Connection` 明示必須(hop-by-hop)、`proxy_read_timeout` 延長。変数 proxy_pass は `resolver`+`$request_uri` 必須。全て nginx.org で確認。
2. **TOPIC B**: `__Host-` は Secure+HTTPS+Domain なし+Path=/ を**すべて**強制(RFC 6265bis MUST / MDN)→ 両バックエンドは**同一オリジン**必須。不透明トークン+共有 Redis 参照ゆえ**署名鍵不要**(JWT なら必要、ZITADEL/Nordic で対比確認)。一致必須なのは sha256 アルゴリズム・キー書式・シリアライズ・TTL。インメモリフォールバックは**プロセスローカルで他系に不可視**=断続ログアウトの原因。
3. **TOPIC C**: 主要アンチパターン=ロールバック無し/ビッグバン/共有 DB 密結合/ファサード恒久化/可観測性欠如、プロキシ SPOF(Fowler・Microsoft・AWS で確認)。シャドウ(nginx `mirror`)は**共有 DB へ書く新バックエンドに送ると二重書き込み**——読み取り専用/冪等/分離ストアに限定。レガシー削除は**検証後の最終ステップ**、それまで旧経路を warm 維持して即ロールバック可能に(Microsoft 公式が明言)。