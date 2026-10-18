-- システムデフォルト Bot（system_default）はシステムの案内役で、MCP・認証情報を使わない（issue #143）。
-- 共有秘書だった頃に付けた利用許可は使われなくなったので消す（DELETE のみ・再実行しても同じ結果）。
DELETE FROM bot_mcp_access WHERE bot_id = 'system_default';
DELETE FROM bot_credential_access WHERE bot_id = 'system_default';
