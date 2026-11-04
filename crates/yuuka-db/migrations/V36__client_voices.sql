-- PWA の音声チャットで使う声（Irodori-TTS の話者条件）。作った本人だけが使い、端末をまたいで使えるよう
-- サーバーにも保存する。声のサンプルの音声そのものは受け取らず、話者エンコーダの出力だけを保存する。
-- speaker_state は f32（リトルエンディアン）の tokens × 768 行列、speaker_mask は長さ tokens の bool の
-- JSON 配列。speaker_model は話者条件を作ったモデルの系統で、モデルを差し替えたら古い声を見分けるのに使う。
CREATE TABLE client_voices (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL,
        name TEXT NOT NULL,
        caption TEXT NOT NULL DEFAULT '',
        source TEXT NOT NULL CHECK (source IN ('sample', 'design')),
        seconds REAL NOT NULL DEFAULT 0,
        speaker_model TEXT NOT NULL,
        tokens INTEGER NOT NULL CHECK (tokens > 0),
        speaker_state BLOB NOT NULL,
        speaker_mask TEXT NOT NULL,
        created_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
        updated_at TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
        FOREIGN KEY (user_id) REFERENCES users(discord_id) ON DELETE CASCADE
      );
CREATE INDEX idx_client_voices_user ON client_voices(user_id, id);
