-- MCP サーバーの OAuth 2.1 認可で得た資格情報（MCP 仕様 Authorization・docs/design/mcp_standard_protocol.md）。
-- rmcp の StoredCredentials（client_id・トークン応答・付与スコープ・発行者）を JSON にして
-- SystemCrypto で暗号化した 3 列で持つ。サーバー削除で一緒に消える。
CREATE TABLE mcp_oauth_credentials (
    server_id INTEGER PRIMARY KEY,
    credentials_encrypted TEXT NOT NULL,
    credentials_iv TEXT NOT NULL,
    credentials_tag TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
    FOREIGN KEY (server_id) REFERENCES mcp_servers(id) ON DELETE CASCADE
);
