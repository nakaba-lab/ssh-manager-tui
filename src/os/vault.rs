//! Encrypted secret store ("the vault") for per-host SSH login passwords and
//! identity-key passphrases.
//!
//! OpenSSH's `~/.ssh/config` has nowhere to put a password, and writing secrets
//! to it in plaintext would be reckless — so the vault lives in its own file
//! (`~/.ssh/sshm-vault.json`), completely separate from the config the rest of
//! the app reads and writes.
//!
//! Layout: a master password is stretched with **Argon2id** into a 32-byte key,
//! which encrypts the serialized entries with **XChaCha20-Poly1305** (AEAD). The
//! salt, nonce and KDF parameters are stored in the clear alongside the
//! ciphertext; the master password itself is never persisted. A wrong password
//! fails the AEAD tag check, surfacing as "incorrect master password" rather
//! than garbage.
//!
//! Like the rest of `os/`, this module has **zero ratatui dependency** and is
//! exercised directly by the unit tests at the bottom of the file.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::aead::Aead;
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

/// On-disk format version. Bumped only if the file layout changes incompatibly.
const VERSION: u32 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24; // XChaCha20-Poly1305 uses a 192-bit nonce.
const KEY_LEN: usize = 32;

/// Which kind of secret an entry holds. A host can have one of each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SecretKind {
    /// A login password for the host (password authentication).
    #[default]
    Password,
    /// The passphrase protecting the host's identity (private) key.
    Passphrase,
}

impl SecretKind {
    pub fn label(self) -> &'static str {
        match self {
            SecretKind::Password => "Password",
            SecretKind::Passphrase => "Passphrase",
        }
    }

    /// The other variant — used by the entry form's toggle.
    pub fn toggled(self) -> Self {
        match self {
            SecretKind::Password => SecretKind::Passphrase,
            SecretKind::Passphrase => SecretKind::Password,
        }
    }
}

/// One stored secret, keyed (loosely) by host alias.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultEntry {
    pub host: String,
    pub kind: SecretKind,
    pub secret: String,
    #[serde(default)]
    pub note: String,
}

/// Argon2id cost parameters, persisted so an existing vault stays openable even
/// if the in-app defaults change later.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct KdfParams {
    m_cost: u32,
    t_cost: u32,
    p_cost: u32,
}

impl Default for KdfParams {
    fn default() -> Self {
        // OWASP-recommended baseline: ~19 MiB, 2 iterations, 1 lane.
        KdfParams {
            m_cost: 19_456,
            t_cost: 2,
            p_cost: 1,
        }
    }
}

/// The serialized on-disk file (all binary fields base64-encoded).
#[derive(Serialize, Deserialize)]
struct VaultFile {
    version: u32,
    kdf: KdfParams,
    salt: String,
    nonce: String,
    ciphertext: String,
}

/// The decrypted plaintext payload (what the ciphertext protects).
#[derive(Default, Serialize, Deserialize)]
struct Payload {
    entries: Vec<VaultEntry>,
}

/// An unlocked vault held in memory for the session. Holds the derived key so
/// edits can be re-saved without re-prompting; the key and secrets are zeroized
/// on drop.
pub struct Vault {
    pub entries: Vec<VaultEntry>,
    key: Zeroizing<Vec<u8>>,
    salt: Vec<u8>,
    params: KdfParams,
}

impl Drop for Vault {
    fn drop(&mut self) {
        for e in &mut self.entries {
            e.secret.zeroize();
        }
        self.salt.zeroize();
    }
}

/// The default vault path, `~/.ssh/sshm-vault.json`.
pub fn default_path() -> Option<PathBuf> {
    super::ssh_dir().map(|d| d.join("sshm-vault.json"))
}

impl Vault {
    /// Create a fresh, empty vault protected by `password` (not yet written to
    /// disk — call [`Vault::save`]).
    pub fn create(password: &str) -> Result<Vault> {
        let params = KdfParams::default();
        let salt = random_bytes(SALT_LEN)?;
        let key = derive_key(password, &salt, &params)?;
        Ok(Vault {
            entries: Vec::new(),
            key,
            salt,
            params,
        })
    }

    /// Decrypt the vault at `path` with `password`. A wrong password is reported
    /// as a clear "incorrect master password" error.
    pub fn unlock(path: &Path, password: &str) -> Result<Vault> {
        let data = fs::read_to_string(path).map_err(|e| anyhow!("cannot read vault file: {e}"))?;
        let file: VaultFile =
            serde_json::from_str(&data).map_err(|e| anyhow!("vault file is corrupt: {e}"))?;
        if file.version != VERSION {
            bail!("unsupported vault version {}", file.version);
        }
        let salt = b64_decode(&file.salt)?;
        let nonce = b64_decode(&file.nonce)?;
        let ciphertext = b64_decode(&file.ciphertext)?;
        if nonce.len() != NONCE_LEN {
            bail!("vault file is corrupt: bad nonce length");
        }
        let key = derive_key(password, &salt, &file.kdf)?;
        let plaintext =
            decrypt(&key, &nonce, &ciphertext).map_err(|_| anyhow!("incorrect master password"))?;
        let payload: Payload = serde_json::from_slice(&plaintext)
            .map_err(|e| anyhow!("vault payload is corrupt: {e}"))?;
        Ok(Vault {
            entries: payload.entries,
            key,
            salt,
            params: file.kdf,
        })
    }

    /// Encrypt and atomically write the vault to `path`.
    pub fn save(&self, path: &Path) -> Result<()> {
        let payload = Payload {
            entries: self.entries.clone(),
        };
        let plaintext = Zeroizing::new(serde_json::to_vec(&payload)?);
        let nonce = random_bytes(NONCE_LEN)?;
        let ciphertext = encrypt(&self.key, &nonce, &plaintext)?;
        let file = VaultFile {
            version: VERSION,
            kdf: self.params,
            salt: B64.encode(&self.salt),
            nonce: B64.encode(&nonce),
            ciphertext: B64.encode(&ciphertext),
        };
        let json = serde_json::to_string_pretty(&file)?;
        atomic_write(path, json.as_bytes())
    }

    /// Replace the entry at `editing` (if any), else append a new one.
    pub fn upsert(&mut self, editing: Option<usize>, entry: VaultEntry) {
        match editing {
            Some(i) if i < self.entries.len() => self.entries[i] = entry,
            _ => self.entries.push(entry),
        }
    }

    /// Remove the entry at `idx`, zeroizing its secret.
    pub fn remove(&mut self, idx: usize) {
        if idx < self.entries.len() {
            let mut e = self.entries.remove(idx);
            e.secret.zeroize();
        }
    }
}

// ---------------------------------------------------------------------------
// Crypto primitives
// ---------------------------------------------------------------------------

fn derive_key(password: &str, salt: &[u8], p: &KdfParams) -> Result<Zeroizing<Vec<u8>>> {
    let params = Params::new(p.m_cost, p.t_cost, p.p_cost, Some(KEY_LEN))
        .map_err(|e| anyhow!("argon2 parameters: {e}"))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = Zeroizing::new(vec![0u8; KEY_LEN]);
    argon
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|e| anyhow!("key derivation failed: {e}"))?;
    Ok(key)
}

fn encrypt(key: &[u8], nonce: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|e| anyhow!("cipher init: {e}"))?;
    cipher
        .encrypt(XNonce::from_slice(nonce), plaintext)
        .map_err(|e| anyhow!("encryption failed: {e}"))
}

fn decrypt(key: &[u8], nonce: &[u8], ciphertext: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|e| anyhow!("cipher init: {e}"))?;
    let pt = cipher
        .decrypt(XNonce::from_slice(nonce), ciphertext)
        .map_err(|_| anyhow!("decryption failed"))?;
    Ok(Zeroizing::new(pt))
}

fn random_bytes(n: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; n];
    getrandom::getrandom(&mut buf).map_err(|e| anyhow!("rng error: {e}"))?;
    Ok(buf)
}

fn b64_decode(s: &str) -> Result<Vec<u8>> {
    B64.decode(s).map_err(|e| anyhow!("base64 decode: {e}"))
}

/// Write `bytes` to `path` atomically (temp file + rename), mirroring the config
/// writer: `0o600` perms on unix, and a remove-before-rename on Windows where
/// `rename` fails if the destination already exists.
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    if !dir.exists() {
        fs::create_dir_all(dir)?;
    }
    let tmp = dir.join(".sshm-vault.tmp");
    fs::write(&tmp, bytes)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
    }

    #[cfg(windows)]
    if path.exists() {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }

    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway path under the OS temp dir, unique to this process+call.
    fn temp_vault_path(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "sshm-vault-test-{}-{tag}-{n}.json",
            std::process::id()
        ))
    }

    fn entry(host: &str, kind: SecretKind, secret: &str) -> VaultEntry {
        VaultEntry {
            host: host.into(),
            kind,
            secret: secret.into(),
            note: String::new(),
        }
    }

    #[test]
    fn create_save_unlock_roundtrip() {
        let path = temp_vault_path("roundtrip");
        let mut v = Vault::create("correct horse").unwrap();
        v.upsert(None, entry("web", SecretKind::Password, "s3cret"));
        v.upsert(None, entry("web", SecretKind::Passphrase, "key-pass"));
        v.save(&path).unwrap();

        let opened = Vault::unlock(&path, "correct horse").unwrap();
        assert_eq!(opened.entries.len(), 2);
        assert_eq!(opened.entries[0].host, "web");
        assert_eq!(opened.entries[0].kind, SecretKind::Password);
        assert_eq!(opened.entries[0].secret, "s3cret");
        assert_eq!(opened.entries[1].kind, SecretKind::Passphrase);
        assert_eq!(opened.entries[1].secret, "key-pass");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn wrong_password_is_rejected() {
        let path = temp_vault_path("wrongpw");
        let mut v = Vault::create("hunter2").unwrap();
        v.upsert(None, entry("db", SecretKind::Password, "p"));
        v.save(&path).unwrap();

        match Vault::unlock(&path, "hunter3") {
            Ok(_) => panic!("wrong password should not unlock the vault"),
            Err(e) => assert!(
                e.to_string().contains("incorrect master password"),
                "unexpected error: {e}"
            ),
        }

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn ciphertext_does_not_leak_the_secret() {
        let path = temp_vault_path("leak");
        let mut v = Vault::create("master").unwrap();
        v.upsert(None, entry("h", SecretKind::Password, "TOPSECRETVALUE"));
        v.save(&path).unwrap();

        let on_disk = fs::read_to_string(&path).unwrap();
        assert!(
            !on_disk.contains("TOPSECRETVALUE"),
            "plaintext secret found in vault file"
        );

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn upsert_replaces_in_place_and_remove_drops() {
        let mut v = Vault::create("m").unwrap();
        v.upsert(None, entry("a", SecretKind::Password, "1"));
        v.upsert(None, entry("b", SecretKind::Password, "2"));
        v.upsert(Some(0), entry("a", SecretKind::Password, "updated"));
        assert_eq!(v.entries[0].secret, "updated");
        assert_eq!(v.entries.len(), 2);
        v.remove(0);
        assert_eq!(v.entries.len(), 1);
        assert_eq!(v.entries[0].host, "b");
    }

    #[test]
    fn unlock_missing_file_errors() {
        let path = temp_vault_path("missing");
        assert!(Vault::unlock(&path, "x").is_err());
    }
}
