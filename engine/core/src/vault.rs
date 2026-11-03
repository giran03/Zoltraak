//! Encrypted-at-rest storage, shared by everything the engine holds on the
//! user's behalf.
//!
//! Two vaults are built on this: [`crate::session::SessionVault`] for browser
//! sessions, and [`SecretVault`] for the standing credentials a user pastes in
//! once — a Discord bot token, a webhook URL. They differ only in what they
//! hold; the protection is identical:
//!
//! - **AES-256-GCM** over a JSON [`Envelope`], with a per-vault AAD so a file
//!   from one vault cannot be swapped in for the other.
//! - The key is **wrapped with DPAPI** on Windows (only the same Windows user
//!   can unwrap it) and kept in an **owner-only file** elsewhere. Copying a
//!   vault file alone yields nothing.
//! - A vault that cannot be decrypted is **treated as empty**, not fatal — see
//!   the note on each vault's `open`.
//!
//! This is defence in depth for a local-only file, not a cross-machine secret
//! store: a process running as the same user could still read the wrapped key,
//! as it could read the browser's own cookie database.

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use zeroize::Zeroizing;

/// The on-disk shape of every vault: base64 nonce and ciphertext, versioned so
/// a future format change can be detected rather than misread.
#[derive(Serialize, Deserialize)]
pub(crate) struct Envelope {
    pub(crate) version: u32,
    pub(crate) nonce: String,
    pub(crate) ciphertext: String,
}

pub(crate) fn encrypt<T: Serialize>(key: &[u8; 32], aad: &[u8], value: &T) -> io::Result<Vec<u8>> {
    use aes_gcm::aead::{Aead, KeyInit, Payload};
    use aes_gcm::{Aes256Gcm, Nonce};
    use base64::Engine;

    let plaintext = Zeroizing::new(serde_json::to_vec(value).map_err(io::Error::other)?);
    let mut nonce = [0u8; 12];
    getrandom::fill(&mut nonce).map_err(|e| io::Error::other(e.to_string()))?;
    let cipher = Aes256Gcm::new(key.into());
    let ciphertext = cipher
        .encrypt(
            &Nonce::from(nonce),
            Payload {
                msg: &plaintext,
                aad,
            },
        )
        .map_err(|_| io::Error::other("vault encryption failed"))?;
    let envelope = Envelope {
        version: 1,
        nonce: base64::engine::general_purpose::STANDARD.encode(nonce),
        ciphertext: base64::engine::general_purpose::STANDARD.encode(ciphertext),
    };
    serde_json::to_vec(&envelope).map_err(io::Error::other)
}

pub(crate) fn decrypt<T: DeserializeOwned>(
    key: &[u8; 32],
    aad: &[u8],
    bytes: &[u8],
) -> io::Result<T> {
    use aes_gcm::aead::{Aead, KeyInit, Payload};
    use aes_gcm::{Aes256Gcm, Nonce};
    use base64::Engine;

    let envelope: Envelope = serde_json::from_slice(bytes).map_err(io::Error::other)?;
    if envelope.version != 1 {
        return Err(io::Error::other(format!(
            "unsupported vault version {}",
            envelope.version
        )));
    }
    let nonce = base64::engine::general_purpose::STANDARD
        .decode(&envelope.nonce)
        .map_err(io::Error::other)?;
    let ciphertext = base64::engine::general_purpose::STANDARD
        .decode(&envelope.ciphertext)
        .map_err(io::Error::other)?;
    let nonce: [u8; 12] = nonce
        .try_into()
        .map_err(|_| io::Error::other("malformed vault nonce"))?;
    let cipher = Aes256Gcm::new(key.into());
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: &ciphertext,
                    aad,
                },
            )
            .map_err(|_| {
                io::Error::other("vault could not be decrypted (wrong key or tampered file)")
            })?,
    );
    serde_json::from_slice(&plaintext).map_err(io::Error::other)
}

const SECRETS_FILE_NAME: &str = "secrets.vault";
const SECRETS_KEY_FILE_NAME: &str = "secrets.key";
const SECRETS_AAD: &[u8] = b"zoltraak-secrets-v1";
const LEGACY_SECRETS_AAD: &[u8] = b"opendm-secrets-v1";

/// Vault keys. Naming them here keeps the string literals in one place and out
/// of the call sites that would otherwise be free to typo them.
pub mod keys {
    pub const DISCORD_BOT_TOKEN: &str = "discord.bot_token";
    pub const DISCORD_WEBHOOK_URL: &str = "discord.webhook_url";
}

/// Long-lived credentials the user configures once: API tokens, webhook URLs.
///
/// Unlike a browser session, none of this can be re-derived — losing the vault
/// means the user pastes the token again — but that is still a better failure
/// than refusing to start, so an unreadable vault is treated as empty here too.
pub struct SecretVault {
    /// `None` for an in-memory vault (tests, or no config directory).
    path: Option<PathBuf>,
    key: Option<Zeroizing<[u8; 32]>>,
    entries: RwLock<HashMap<String, String>>,
}

impl fmt::Debug for SecretVault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretVault")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl Default for SecretVault {
    fn default() -> Self {
        Self::in_memory()
    }
}

impl SecretVault {
    /// A vault that forgets everything when the process exits.
    pub fn in_memory() -> Self {
        Self {
            path: None,
            key: None,
            entries: RwLock::new(HashMap::new()),
        }
    }

    /// Open (or create) the secret vault in `dir`. A file that cannot be
    /// decrypted — a lost key, another user's profile, tampering — is logged
    /// and treated as empty: the user re-enters the credential, which is a far
    /// better outcome than the engine refusing to start.
    pub fn open(dir: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let key = key_store::load_or_create(&dir.join(SECRETS_KEY_FILE_NAME))?;
        let path = dir.join(SECRETS_FILE_NAME);
        let entries: HashMap<String, String> = match std::fs::read(&path) {
            Ok(bytes) => match decrypt(&key, SECRETS_AAD, &bytes)
                .or_else(|_| decrypt(&key, LEGACY_SECRETS_AAD, &bytes))
            {
                Ok(map) => map,
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "secret vault unreadable; starting empty");
                    HashMap::new()
                }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => return Err(e),
        };
        tracing::info!(path = %path.display(), secrets = entries.len(), "secret vault opened");
        Ok(Self {
            path: Some(path),
            key: Some(key),
            entries: RwLock::new(entries),
        })
    }

    pub fn is_persistent(&self) -> bool {
        self.path.is_some()
    }

    /// Re-read secrets from disk if modified by another process.
    pub async fn reload(&self) -> io::Result<bool> {
        let (Some(path), Some(key)) = (&self.path, &self.key) else {
            return Ok(false);
        };
        let fresh: HashMap<String, String> = match std::fs::read(path) {
            Ok(bytes) => match decrypt(key, SECRETS_AAD, &bytes)
                .or_else(|_| decrypt(key, LEGACY_SECRETS_AAD, &bytes))
            {
                Ok(map) => map,
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "secret vault reload failed");
                    return Ok(false);
                }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        };
        let mut entries = self.entries.write().await;
        *entries = fresh;
        Ok(true)
    }

    pub async fn get(&self, key: &str) -> Option<String> {
        let _ = self.reload().await;
        self.entries.read().await.get(key).cloned()
    }

    pub async fn has(&self, key: &str) -> bool {
        self.entries.read().await.contains_key(key)
    }

    /// What a UI may show about a stored secret: enough to tell two tokens
    /// apart, never enough to use one. `None` when nothing is stored.
    pub async fn masked(&self, key: &str) -> Option<String> {
        self.entries.read().await.get(key).map(|v| mask(v))
    }

    /// Store a secret. An empty (or whitespace-only) value removes it instead:
    /// clearing the field in the UI is how a user disconnects.
    pub async fn set(&self, key: &str, value: &str) -> io::Result<()> {
        let value = value.trim();
        let mut entries = self.entries.write().await;
        if value.is_empty() {
            entries.remove(key);
        } else {
            entries.insert(key.to_string(), value.to_string());
        }
        self.persist(&entries)
    }

    pub async fn remove(&self, key: &str) -> io::Result<bool> {
        let mut entries = self.entries.write().await;
        let removed = entries.remove(key).is_some();
        if removed {
            self.persist(&entries)?;
        }
        Ok(removed)
    }

    pub async fn clear(&self) -> io::Result<()> {
        let mut entries = self.entries.write().await;
        entries.clear();
        if let Some(path) = &self.path {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn persist(&self, entries: &HashMap<String, String>) -> io::Result<()> {
        let (Some(path), Some(key)) = (&self.path, &self.key) else {
            return Ok(());
        };
        if entries.is_empty() {
            return match std::fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e),
            };
        }
        let bytes = encrypt(key, SECRETS_AAD, entries)?;
        // Write-then-rename so a crash mid-write leaves the previous vault,
        // not half of a new one.
        let tmp = path.with_extension("vault.tmp");
        key_store::write_private(&tmp, &bytes)?;
        std::fs::OpenOptions::new()
            .write(true)
            .open(&tmp)?
            .sync_all()?;
        std::fs::rename(&tmp, path)
    }
}

/// The last four characters, and nothing else. A value short enough that four
/// characters would give away most of it shows as all dots instead.
pub fn mask(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    let tail: String = if chars.len() <= 8 {
        String::new()
    } else {
        chars[chars.len() - 4..].iter().collect()
    };
    format!("••••{tail}")
}

/// Where a vault key lives, and how it is protected on each platform.
pub(crate) mod key_store {
    use std::io;
    use std::path::Path;

    use zeroize::Zeroizing;

    pub fn load_or_create(path: &Path) -> io::Result<Zeroizing<[u8; 32]>> {
        match std::fs::read(path) {
            Ok(wrapped) => match unwrap_key(&wrapped) {
                Ok(key) => return Ok(key),
                Err(e) => return Err(e),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let mut key = Zeroizing::new([0u8; 32]);
        getrandom::fill(&mut *key).map_err(|e| io::Error::other(e.to_string()))?;
        write_private(path, &wrap_key(&key)?)?;
        Ok(key)
    }

    /// Write with an owner-only mode where the OS has one, set at create
    /// time so there is no window in which the file is world-readable.
    #[cfg(unix)]
    pub fn write_private(path: &Path, contents: &[u8]) -> io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(contents)
    }

    /// Windows has no mode bits; the file lands under the user's profile,
    /// which is ACL'd to that user, and the key inside is DPAPI-wrapped so
    /// even a copied file is useless to another account.
    #[cfg(not(unix))]
    pub fn write_private(path: &Path, contents: &[u8]) -> io::Result<()> {
        std::fs::write(path, contents)
    }

    /// Protect the key with the Windows Data Protection API: the blob can
    /// only be unwrapped by the same Windows user on the same machine.
    #[cfg(windows)]
    fn wrap_key(key: &[u8; 32]) -> io::Result<Vec<u8>> {
        dpapi::protect(key)
    }

    #[cfg(windows)]
    fn unwrap_key(wrapped: &[u8]) -> io::Result<Zeroizing<[u8; 32]>> {
        let raw = dpapi::unprotect(wrapped)?;
        if raw.len() != 32 {
            return Err(io::Error::other("unwrapped vault key has the wrong length"));
        }
        let mut key = Zeroizing::new([0u8; 32]);
        key.copy_from_slice(&raw);
        Ok(key)
    }

    /// Elsewhere the key file's owner-only permissions are the protection.
    #[cfg(not(windows))]
    fn wrap_key(key: &[u8; 32]) -> io::Result<Vec<u8>> {
        Ok(key.to_vec())
    }

    #[cfg(not(windows))]
    fn unwrap_key(wrapped: &[u8]) -> io::Result<Zeroizing<[u8; 32]>> {
        if wrapped.len() != 32 {
            return Err(io::Error::other("vault key file has the wrong length"));
        }
        let mut key = Zeroizing::new([0u8; 32]);
        key.copy_from_slice(wrapped);
        Ok(key)
    }

    #[cfg(windows)]
    mod dpapi {
        use std::io;

        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        };

        /// Extra entropy mixed into the wrap, so a blob produced by another
        /// program under the same account does not decrypt as ours.
        ///
        /// The string still says "session" because changing it would orphan
        /// every key file already written under the old value.
        const ENTROPY: &[u8] = b"Zoltraak session vault key v1";

        fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
            CRYPT_INTEGER_BLOB {
                cbData: data.len() as u32,
                pbData: data.as_ptr() as *mut u8,
            }
        }

        pub fn protect(data: &[u8]) -> io::Result<Vec<u8>> {
            let input = blob(data);
            let entropy = blob(ENTROPY);
            let mut output = CRYPT_INTEGER_BLOB {
                cbData: 0,
                pbData: std::ptr::null_mut(),
            };
            // SAFETY: the input and entropy blobs point at live slices for the
            // duration of the call; the output blob is filled by the API and
            // freed with LocalFree below.
            let ok = unsafe {
                CryptProtectData(
                    &input,
                    std::ptr::null(),
                    &entropy,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output,
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: on success the API guarantees pbData points at cbData bytes.
            let bytes = unsafe {
                std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec()
            };
            unsafe { LocalFree(output.pbData as _) };
            Ok(bytes)
        }

        pub fn unprotect(data: &[u8]) -> io::Result<Vec<u8>> {
            let input = blob(data);
            let entropy = blob(ENTROPY);
            let mut output = CRYPT_INTEGER_BLOB {
                cbData: 0,
                pbData: std::ptr::null_mut(),
            };
            // SAFETY: as in `protect`.
            let ok = unsafe {
                CryptUnprotectData(
                    &input,
                    std::ptr::null_mut(),
                    &entropy,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output,
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: on success the API guarantees pbData points at cbData bytes.
            let bytes = unsafe {
                std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec()
            };
            unsafe { LocalFree(output.pbData as _) };
            Ok(bytes)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_secret_survives_a_reopen_and_never_touches_disk_in_the_clear() {
        let dir = tempfile::tempdir().unwrap();
        let vault = SecretVault::open(dir.path()).unwrap();
        vault
            .set(keys::DISCORD_BOT_TOKEN, "MTIzNDU2.abcdef.supersecrettoken")
            .await
            .unwrap();

        let on_disk = std::fs::read(dir.path().join("secrets.vault")).unwrap();
        assert!(
            !String::from_utf8_lossy(&on_disk).contains("supersecrettoken"),
            "token leaked to disk"
        );

        let reopened = SecretVault::open(dir.path()).unwrap();
        assert_eq!(
            reopened.get(keys::DISCORD_BOT_TOKEN).await.as_deref(),
            Some("MTIzNDU2.abcdef.supersecrettoken")
        );
        assert!(reopened.has(keys::DISCORD_BOT_TOKEN).await);
        assert!(!reopened.has(keys::DISCORD_WEBHOOK_URL).await);
    }

    #[tokio::test]
    async fn a_tampered_vault_is_treated_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let vault = SecretVault::open(dir.path()).unwrap();
        vault
            .set(
                keys::DISCORD_WEBHOOK_URL,
                "https://discord.com/api/webhooks/1/abc",
            )
            .await
            .unwrap();
        drop(vault);

        let path = dir.path().join("secrets.vault");
        let mut envelope: Envelope =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let mut chars: Vec<char> = envelope.ciphertext.chars().collect();
        chars[5] = if chars[5] == 'A' { 'B' } else { 'A' };
        envelope.ciphertext = chars.into_iter().collect();
        std::fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();

        let reopened = SecretVault::open(dir.path()).unwrap();
        assert!(reopened.get(keys::DISCORD_WEBHOOK_URL).await.is_none());
    }

    #[test]
    fn a_session_vault_file_cannot_be_read_as_a_secret_vault() {
        // Same key, different AAD: swapping one vault's file in for the
        // other's must fail closed rather than decrypt into the wrong shape.
        let key = Zeroizing::new([7u8; 32]);
        let sessions: HashMap<String, String> = HashMap::from([("a".to_string(), "b".to_string())]);
        let bytes = encrypt(&key, b"zoltraak-sessions-v1", &sessions).unwrap();
        assert!(decrypt::<HashMap<String, String>>(&key, SECRETS_AAD, &bytes).is_err());
    }

    #[tokio::test]
    async fn clearing_a_value_removes_it_and_emptying_the_vault_deletes_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let vault = SecretVault::open(dir.path()).unwrap();
        vault
            .set(keys::DISCORD_BOT_TOKEN, "token-value-here")
            .await
            .unwrap();
        vault.set(keys::DISCORD_BOT_TOKEN, "   ").await.unwrap();
        assert!(
            vault.get(keys::DISCORD_BOT_TOKEN).await.is_none(),
            "a blank value disconnects"
        );
        assert!(
            !dir.path().join("secrets.vault").exists(),
            "the last secret takes the file with it"
        );

        vault
            .set(keys::DISCORD_WEBHOOK_URL, "https://example.invalid/hook")
            .await
            .unwrap();
        assert!(vault.remove(keys::DISCORD_WEBHOOK_URL).await.unwrap());
        assert!(!vault.remove(keys::DISCORD_WEBHOOK_URL).await.unwrap());
    }

    #[tokio::test]
    async fn masking_shows_a_tail_and_nothing_more() {
        let vault = SecretVault::in_memory();
        vault
            .set(keys::DISCORD_BOT_TOKEN, "MTIzNDU2Nzg5.GhIjKl.mnopqr9z")
            .await
            .unwrap();
        assert_eq!(
            vault.masked(keys::DISCORD_BOT_TOKEN).await.as_deref(),
            Some("••••qr9z")
        );
        assert_eq!(
            mask("short"),
            "••••",
            "too short to show a tail without giving it away"
        );
        assert!(!mask("MTIzNDU2Nzg5.GhIjKl.mnopqr9z").contains("MTIz"));
        assert!(vault.masked(keys::DISCORD_WEBHOOK_URL).await.is_none());
    }

    #[test]
    fn debug_output_never_shows_a_secret() {
        let rendered = format!("{:?}", SecretVault::in_memory());
        assert!(!rendered.contains("token"));
    }
}
