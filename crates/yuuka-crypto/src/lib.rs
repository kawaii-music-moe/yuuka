//! yuuka-crypto — 保存時（at-rest）暗号化。**Node `src/utils/crypto.ts` の 1:1 パリティ**。
//!
//! 2 種の鍵で AES-256-GCM 暗号化する:
//! - **システム鍵**: `scrypt(secret, SYSTEM_SALT)`（Node `crypto.scryptSync` 既定 N=16384/r=8/p=1）。
//!   API キー・Discord トークン・OAuth トークン・Webhook/MCP シークレット等に使う。
//! - **ユーザー鍵**: `Argon2id(secret, userSalt)`（Node `@node-rs/argon2` `hashRawSync`
//!   m=19456/t=2/p=1/len=32）。パスワードマネージャ（`credentials` 表）専用。
//!
//! 暗号文・IV・authTag は Node と同じ **hex 文字列**で相互運用する（`encrypted`/`iv`/`auth_tag`）。
//! GCM は 12 バイト IV・16 バイト authTag。RustCrypto の `encrypt` は `ciphertext||tag` を返すため、
//! 末尾 16 バイトを tag として分離して Node のカラム分割（`iv`/`auth_tag`）に合わせる。
//!
//! DAG: `crypto → core`（config から秘密を受け取る）。DB 依存はローテーション（[`rotate`]）のみ。

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use argon2::{Algorithm, Argon2, Params as Argon2Params, Version};
use scrypt::{scrypt, Params as ScryptParams};
use secrecy::{ExposeSecret, SecretString};

pub mod rotate;

pub use rotate::{rotate_secret_key, EncryptedColumnSpec, ENCRYPTED_COLUMNS};

/// 暗号鍵を導出するための固定ソルト（Node `SYSTEM_SALT`・**後方互換のため変更不可**）。
pub const SYSTEM_SALT: &str = "yuuka-seminar-accounting-salt";

/// プレリリース版が `YUUKA_ENCRYPTION_SECRET` 未設定時に使っていた鍵（Node `LEGACY_FALLBACK_SECRET`）。
/// ローテーション（旧鍵からの移行）でのみ参照する。
pub const LEGACY_FALLBACK_SECRET: &str = "yuuka-seminar-2026-system-key";

/// GCM の IV（ノンス）長。GCM 推奨の 12 バイト（Node `crypto.randomBytes(12)`）。
const IV_LEN: usize = 12;
/// GCM の authTag 長（16 バイト固定）。
const TAG_LEN: usize = 16;
/// 導出鍵長（AES-256 = 32 バイト）。
const KEY_LEN: usize = 32;

/// 暗号層のエラー。復号失敗（鍵不一致・データ破損）は per-item であって致命ではない。
/// 秘密未設定（`SecretMissing`）は fail-closed（機能停止）だが即プロセス終了ではない。
#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    /// `YUUKA_ENCRYPTION_SECRET` 未設定（Node `getEncryptionKey` の throw 相当）。
    #[error("YUUKA_ENCRYPTION_SECRET が設定されていません（at-rest 暗号化に必須）")]
    SecretMissing,
    /// scrypt / Argon2 の鍵導出失敗（パラメータ不正・出力長不一致等）。
    #[error("鍵導出に失敗しました: {0}")]
    KeyDerivation(String),
    /// ユーザーソルトが不正（hex でない／8 バイト未満・Node の salt 検証と同一）。
    #[error("ユーザーソルトが不正です（8 バイト以上の hex が必要）")]
    BadSalt,
    /// IV が不正（hex でない／12 バイトでない）。
    #[error("IV が不正です（12 バイトの hex が必要）")]
    BadIv,
    /// authTag が不正（hex でない／16 バイトでない）。
    #[error("authTag が不正です（16 バイトの hex が必要）")]
    BadAuthTag,
    /// 暗号文が hex でない。
    #[error("暗号文の hex デコードに失敗しました")]
    BadCiphertext,
    /// AES-256-GCM の暗号化失敗（実質発生しないが握り潰さない）。
    #[error("暗号化に失敗しました")]
    Encrypt,
    /// AES-256-GCM の復号／認証失敗（鍵不一致・改竄・データ破損）。
    #[error("復号に失敗しました（鍵不一致またはデータ破損）")]
    Decrypt,
    /// 復号結果が UTF-8 でない（想定外・データ破損）。
    #[error("復号結果が UTF-8 として不正です")]
    NotUtf8,
    /// CSPRNG から IV を取得できなかった。
    #[error("乱数生成に失敗しました: {0}")]
    Random(String),
    /// ローテーション中の DB エラー（[`rotate`]）。
    #[error("鍵ローテーションの DB 操作に失敗しました: {0}")]
    Db(#[from] rusqlite::Error),
}

/// AES-256-GCM の暗号化結果。全て hex 文字列で、Node のカラム（`encrypted`/`iv`/`auth_tag`）と一致する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encrypted {
    /// 暗号文（hex）。
    pub encrypted: String,
    /// IV / ノンス（12 バイトの hex）。
    pub iv: String,
    /// authTag（16 バイトの hex）。
    pub auth_tag: String,
}

/// システム鍵を導出する（`scrypt(secret, SYSTEM_SALT, N=16384/r=8/p=1, 32)`）。
///
/// # Errors
/// scrypt パラメータ不正・出力長不一致で [`CryptoError::KeyDerivation`]。
pub fn derive_system_key(secret: &str) -> Result<[u8; KEY_LEN], CryptoError> {
    // log_n = log2(16384) = 14。Node scryptSync 既定（N=16384, r=8, p=1, keylen=32）と一致。
    let params = ScryptParams::new(14, 8, 1, KEY_LEN)
        .map_err(|e| CryptoError::KeyDerivation(e.to_string()))?;
    let mut key = [0u8; KEY_LEN];
    scrypt(secret.as_bytes(), SYSTEM_SALT.as_bytes(), &params, &mut key)
        .map_err(|e| CryptoError::KeyDerivation(e.to_string()))?;
    Ok(key)
}

/// ユーザー鍵を導出する（`Argon2id(secret, salt, m=19456/t=2/p=1, len=32)`）。
///
/// `salt_hex` は `users.salt`（CSPRNG 生成の hex）。8 バイト未満は拒否（Node と同一）。
///
/// # Errors
/// ソルト不正で [`CryptoError::BadSalt`]、Argon2 失敗で [`CryptoError::KeyDerivation`]。
pub fn derive_user_key(secret: &str, salt_hex: &str) -> Result<[u8; KEY_LEN], CryptoError> {
    let salt = hex::decode(salt_hex).map_err(|_| CryptoError::BadSalt)?;
    if salt.len() < 8 {
        return Err(CryptoError::BadSalt);
    }
    // OWASP 推奨最小構成（Node ARGON2_OPTS と同一）: memoryCost=19456 KiB, timeCost=2, parallelism=1。
    let params = Argon2Params::new(19456, 2, 1, Some(KEY_LEN))
        .map_err(|e| CryptoError::KeyDerivation(e.to_string()))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; KEY_LEN];
    argon
        .hash_password_into(secret.as_bytes(), &salt, &mut key)
        .map_err(|e| CryptoError::KeyDerivation(e.to_string()))?;
    Ok(key)
}

/// 導出済み鍵で AES-256-GCM 暗号化する（IV は毎回 CSPRNG で 12 バイト生成）。
///
/// # Errors
/// 乱数取得失敗で [`CryptoError::Random`]、暗号化失敗で [`CryptoError::Encrypt`]。
pub fn encrypt_with_key(key: &[u8; KEY_LEN], plaintext: &str) -> Result<Encrypted, CryptoError> {
    let mut iv = [0u8; IV_LEN];
    getrandom::getrandom(&mut iv).map_err(|e| CryptoError::Random(e.to_string()))?;

    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce = Nonce::from_slice(&iv);
    // RustCrypto の encrypt は ciphertext||tag を返す。末尾 TAG_LEN を tag として分離。
    let ct_and_tag = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|_| CryptoError::Encrypt)?;
    if ct_and_tag.len() < TAG_LEN {
        return Err(CryptoError::Encrypt);
    }
    let (ciphertext, tag) = ct_and_tag.split_at(ct_and_tag.len() - TAG_LEN);
    Ok(Encrypted {
        encrypted: hex::encode(ciphertext),
        iv: hex::encode(iv),
        auth_tag: hex::encode(tag),
    })
}

/// 導出済み鍵で AES-256-GCM 復号する（Node の hex カラム 3 つを受け取る）。
///
/// # Errors
/// hex/長さ不正で `Bad*`、認証失敗で [`CryptoError::Decrypt`]、非 UTF-8 で [`CryptoError::NotUtf8`]。
pub fn decrypt_with_key(
    key: &[u8; KEY_LEN],
    encrypted_hex: &str,
    iv_hex: &str,
    auth_tag_hex: &str,
) -> Result<String, CryptoError> {
    let iv = hex::decode(iv_hex).map_err(|_| CryptoError::BadIv)?;
    if iv.len() != IV_LEN {
        return Err(CryptoError::BadIv);
    }
    let tag = hex::decode(auth_tag_hex).map_err(|_| CryptoError::BadAuthTag)?;
    if tag.len() != TAG_LEN {
        return Err(CryptoError::BadAuthTag);
    }
    let mut buf = hex::decode(encrypted_hex).map_err(|_| CryptoError::BadCiphertext)?;
    // RustCrypto の decrypt は ciphertext||tag を 1 バッファで受ける。
    buf.extend_from_slice(&tag);

    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce = Nonce::from_slice(&iv);
    let plain = cipher
        .decrypt(nonce, buf.as_ref())
        .map_err(|_| CryptoError::Decrypt)?;
    String::from_utf8(plain).map_err(|_| CryptoError::NotUtf8)
}

/// ユーザー登録時のソルト生成（CSPRNG・16 バイト hex＝Node `generateUserSalt` と同一形式）。
///
/// # Errors
/// 乱数取得失敗で [`CryptoError::Random`]。
pub fn generate_user_salt() -> Result<String, CryptoError> {
    let mut salt = [0u8; 16];
    getrandom::getrandom(&mut salt).map_err(|e| CryptoError::Random(e.to_string()))?;
    Ok(hex::encode(salt))
}

/// システム鍵を保持し、システム／ユーザー両方の暗号化を提供するハンドル。
///
/// アプリ起動時に config から 1 つ構築して共有する。`secret` は redact 保持（`Debug` で漏れない）。
/// ユーザー鍵は都度 Argon2id 導出すると重い（19 MiB）ため、ソルト単位でメモリキャッシュする。
pub struct SystemCrypto {
    /// scrypt 導出済みシステム鍵。
    system_key: [u8; KEY_LEN],
    /// ユーザー鍵導出のためのマスタ秘密（`YUUKA_ENCRYPTION_SECRET`）。redact 保持。
    secret: SecretString,
    /// `salt_hex → user_key` のメモリキャッシュ（Node の `userKeyCache` 相当）。
    user_key_cache: Mutex<HashMap<String, [u8; KEY_LEN]>>,
}

impl std::fmt::Debug for SystemCrypto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 鍵・秘密は絶対に出さない。
        f.debug_struct("SystemCrypto").finish_non_exhaustive()
    }
}

impl SystemCrypto {
    /// マスタ秘密からシステム鍵を導出して構築する。
    ///
    /// # Errors
    /// scrypt 失敗で [`CryptoError::KeyDerivation`]。
    pub fn new(secret: SecretString) -> Result<Self, CryptoError> {
        let system_key = derive_system_key(secret.expose_secret())?;
        Ok(Self {
            system_key,
            secret,
            user_key_cache: Mutex::new(HashMap::new()),
        })
    }

    /// config の `YUUKA_ENCRYPTION_SECRET` から構築する。未設定なら [`CryptoError::SecretMissing`]。
    ///
    /// # Errors
    /// 秘密未設定で [`CryptoError::SecretMissing`]、導出失敗で [`CryptoError::KeyDerivation`]。
    pub fn from_config(cfg: &yuuka_core::Config) -> Result<Self, CryptoError> {
        let secret = cfg
            .encryption_secret
            .clone()
            .ok_or(CryptoError::SecretMissing)?;
        Self::new(secret)
    }

    /// システム鍵で暗号化する（API キー・Discord トークン・OAuth トークン・Webhook/MCP 秘密）。
    ///
    /// # Errors
    /// [`encrypt_with_key`] 参照。
    pub fn encrypt_text(&self, plaintext: &str) -> Result<Encrypted, CryptoError> {
        encrypt_with_key(&self.system_key, plaintext)
    }

    /// システム鍵で復号する。
    ///
    /// # Errors
    /// [`decrypt_with_key`] 参照。
    pub fn decrypt_text(
        &self,
        encrypted_hex: &str,
        iv_hex: &str,
        auth_tag_hex: &str,
    ) -> Result<String, CryptoError> {
        decrypt_with_key(&self.system_key, encrypted_hex, iv_hex, auth_tag_hex)
    }

    /// ユーザー固有鍵を取得する（キャッシュ付き・`salt_hex` 単位）。
    fn user_key(&self, salt_hex: &str) -> Result<[u8; KEY_LEN], CryptoError> {
        // ロック毒化は inner を回収して継続（panic 伝播させない・絶対制約1）。
        {
            let cache = self
                .user_key_cache
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(k) = cache.get(salt_hex) {
                return Ok(*k);
            }
        }
        let key = derive_user_key(self.secret.expose_secret(), salt_hex)?;
        let mut cache = self
            .user_key_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        cache.insert(salt_hex.to_owned(), key);
        Ok(key)
    }

    /// ユーザー固有鍵で暗号化する（パスワードマネージャ専用）。
    ///
    /// # Errors
    /// ソルト不正・導出失敗・暗号化失敗で [`CryptoError`]。
    pub fn encrypt_for_user(
        &self,
        salt_hex: &str,
        plaintext: &str,
    ) -> Result<Encrypted, CryptoError> {
        let key = self.user_key(salt_hex)?;
        encrypt_with_key(&key, plaintext)
    }

    /// ユーザー固有鍵で復号する（パスワードマネージャ専用）。
    ///
    /// # Errors
    /// ソルト不正・導出失敗・復号失敗で [`CryptoError`]。
    pub fn decrypt_for_user(
        &self,
        salt_hex: &str,
        encrypted_hex: &str,
        iv_hex: &str,
        auth_tag_hex: &str,
    ) -> Result<String, CryptoError> {
        let key = self.user_key(salt_hex)?;
        decrypt_with_key(&key, encrypted_hex, iv_hex, auth_tag_hex)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_key_is_deterministic_and_secret_dependent() {
        let a = derive_system_key("hunter2").unwrap();
        let b = derive_system_key("hunter2").unwrap();
        let c = derive_system_key("different").unwrap();
        assert_eq!(a, b, "同一秘密は同一鍵（決定的）");
        assert_ne!(a, c, "秘密が違えば鍵も違う");
        assert_eq!(a.len(), 32);
    }

    #[test]
    fn system_roundtrip() {
        let key = derive_system_key("master-secret").unwrap();
        let enc = encrypt_with_key(&key, "gemini-api-key-XYZ").unwrap();
        // IV=12B→24hex、tag=16B→32hex。
        assert_eq!(enc.iv.len(), 24);
        assert_eq!(enc.auth_tag.len(), 32);
        let back = decrypt_with_key(&key, &enc.encrypted, &enc.iv, &enc.auth_tag).unwrap();
        assert_eq!(back, "gemini-api-key-XYZ");
    }

    #[test]
    fn wrong_key_fails_auth_not_panic() {
        let key = derive_system_key("k1").unwrap();
        let other = derive_system_key("k2").unwrap();
        let enc = encrypt_with_key(&key, "secret").unwrap();
        let err = decrypt_with_key(&other, &enc.encrypted, &enc.iv, &enc.auth_tag).unwrap_err();
        assert!(matches!(err, CryptoError::Decrypt));
    }

    #[test]
    fn user_roundtrip_and_salt_isolation() {
        let salt_a = generate_user_salt().unwrap();
        let salt_b = generate_user_salt().unwrap();
        assert_ne!(salt_a, salt_b);
        let crypto = SystemCrypto::new(SecretString::from("master")).unwrap();
        let enc = crypto.encrypt_for_user(&salt_a, "p@ssw0rd").unwrap();
        assert_eq!(
            crypto
                .decrypt_for_user(&salt_a, &enc.encrypted, &enc.iv, &enc.auth_tag)
                .unwrap(),
            "p@ssw0rd"
        );
        // 別ソルトの鍵では復号できない（ユーザー分離）。
        assert!(crypto
            .decrypt_for_user(&salt_b, &enc.encrypted, &enc.iv, &enc.auth_tag)
            .is_err());
    }

    #[test]
    fn bad_inputs_are_rejected_not_panicked() {
        let key = derive_system_key("k").unwrap();
        // 非 hex IV。
        assert!(matches!(
            decrypt_with_key(&key, "00", "zz", "00").unwrap_err(),
            CryptoError::BadIv
        ));
        // 短すぎるソルト（4 バイト）。
        assert!(matches!(
            derive_user_key("s", "abcd").unwrap_err(),
            CryptoError::BadSalt
        ));
    }

    /// Node（`src/utils/crypto.ts` + `@node-rs/argon2`）が実際に出力したゴールデンベクタで、
    /// scrypt/Argon2 のパラメータと AES-256-GCM のカラム分割（ciphertext / iv / authTag）が
    /// **バイト単位で Node と一致**することを凍結する。ここが緑なら既存 DB の暗号化データを
    /// そのまま復号でき、Rust が書いた暗号文を Node も復号できる（相互運用）。
    #[test]
    fn golden_parity_with_node() {
        const SECRET: &str = "golden-master-secret-123";
        const SALT: &str = "0011223344556677889900aabbccddee";

        // 1) システム鍵導出（Node crypto.scryptSync 既定パラメータ）。
        let sys_key = derive_system_key(SECRET).unwrap();
        assert_eq!(
            hex::encode(sys_key),
            "c1d1a8f32ae712b3d17bf0a4ae2e363d081a682eaf6ae0e2426aa1e6e7d0d7e2",
            "scrypt 導出鍵が Node と不一致（パラメータ N=16384/r=8/p=1 を確認）"
        );
        // 2) システム鍵で暗号化した Node の暗号文を復号できる（GCM カラム分割の一致）。
        assert_eq!(
            decrypt_with_key(
                &sys_key,
                "4ccb56445018fa64886cfa24aba88696f754e0cdb09f452a292cfebe1ee260",
                "79dafa6759fa1b917eeaf220",
                "4f93e7447f857f318263073643c15366",
            )
            .unwrap(),
            "gemini-api-key-ABCDEF-日本語"
        );

        // 3) ユーザー鍵導出（Node @node-rs/argon2 hashRawSync・Argon2id m=19456/t=2/p=1）。
        let user_key = derive_user_key(SECRET, SALT).unwrap();
        assert_eq!(
            hex::encode(user_key),
            "419d3cef8a5c6cdf8a1bf4a8fffa4f4a373703f0e35a89f006529ea7335a83bb",
            "Argon2id 導出鍵が Node と不一致（バージョン 0x13・m=19456/t=2/p=1 を確認）"
        );
        // 4) ユーザー鍵で暗号化した Node の暗号文を復号できる。
        assert_eq!(
            decrypt_with_key(
                &user_key,
                "a42d387394a9516a689dd5a42df6e937dba88ece",
                "2740d99bc64cb39b9d2123c7",
                "8dbba0f023ee7e3e6ff5e659b17fe1c3",
            )
            .unwrap(),
            "s3cr3t-p@ssw0rd-🔐"
        );
    }

    #[test]
    fn missing_secret_is_fail_closed() {
        // encryption_secret 未設定の config からは SecretMissing。
        let cfg = yuuka_core::Config::load_and_validate(std::path::Path::new("/no/such/cfg"))
            .expect("defaults");
        assert!(matches!(
            SystemCrypto::from_config(&cfg).unwrap_err(),
            CryptoError::SecretMissing
        ));
    }
}
