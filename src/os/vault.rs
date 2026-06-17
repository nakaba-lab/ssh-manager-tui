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
//! ciphertext, but they are bound into the AEAD as **associated data**, so a
//! tampered header (version / KDF cost / salt) fails the tag rather than silently
//! changing the key. The KDF parameters read from disk are also **range-checked**
//! before use, so a hostile file cannot force an unbounded Argon2 allocation
//! (DoS) or weaken the KDF below a safe floor. The master password itself is
//! never persisted; a wrong password fails the AEAD tag and surfaces as
//! "incorrect master password".
//!
//! Secret material is wrapped in [`Secret`] (scrubbed on drop, redacted in
//! `Debug`) and the derived key / decrypted plaintext live in `Zeroizing`
//! buffers. Like the rest of `os/`, this module has **zero ratatui dependency**
//! and is exercised by the unit tests at the bottom of the file.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::aead::{Aead, Payload as AeadPayload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

/// On-disk format version. Bumped only if the file layout changes incompatibly.
const VERSION: u32 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24; // XChaCha20-Poly1305 uses a 192-bit nonce.
const KEY_LEN: usize = 32;

// KDF parameter bounds, enforced on unlock against the untrusted file: a tampered
// file can neither force an unbounded memory allocation (DoS) nor weaken the KDF
// below a safe floor (downgrade). The defaults below sit inside this window.
const MIN_M_COST: u32 = 8 * 1024; // 8 MiB floor
const MAX_M_COST: u32 = 1024 * 1024; // 1 GiB cap
const MIN_T_COST: u32 = 1;
const MAX_T_COST: u32 = 16;
const MIN_P_COST: u32 = 1;
const MAX_P_COST: u32 = 16;

/// A secret string that scrubs its backing buffer on drop and never reveals
/// itself in `Debug`. Serializes transparently as a plain string, so it is the
/// *ciphertext* — never this — that reaches disk. Clones (and the buffers freed
/// as one is edited) zeroize themselves, so plaintext does not accumulate on the
/// heap the way a bare `String` would.
#[derive(Clone, Default)]
pub struct Secret(String);

impl Secret {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::ops::Deref for Secret {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl From<String> for Secret {
    fn from(s: String) -> Self {
        Secret(s)
    }
}

impl From<&str> for Secret {
    fn from(s: &str) -> Self {
        Secret(s.to_owned())
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("\"***\"")
    }
}

impl Serialize for Secret {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Secret(String::deserialize(d)?))
    }
}

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
    pub secret: Secret,
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

impl KdfParams {
    /// Reject parameters from an untrusted file that are unreasonably large
    /// (allocation DoS) or below the safe floor (KDF downgrade).
    fn validate(&self) -> Result<()> {
        if !(MIN_M_COST..=MAX_M_COST).contains(&self.m_cost)
            || !(MIN_T_COST..=MAX_T_COST).contains(&self.t_cost)
            || !(MIN_P_COST..=MAX_P_COST).contains(&self.p_cost)
        {
            bail!("vault file is corrupt: unreasonable KDF parameters");
        }
        Ok(())
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

/// The decrypted plaintext payload, owned (used when deserializing).
#[derive(Default, Deserialize)]
struct Payload {
    entries: Vec<VaultEntry>,
}

/// A borrowing view of the payload used when serializing, so `save` never clones
/// the plaintext secrets into a second heap allocation.
#[derive(Serialize)]
struct PayloadRef<'a> {
    entries: &'a [VaultEntry],
}

/// An unlocked vault held in memory for the session. Holds the derived key so
/// edits can be re-saved without re-prompting; the key (Zeroizing) and entry
/// secrets (Secret) scrub themselves on drop, and the salt is zeroized here.
pub struct Vault {
    pub entries: Vec<VaultEntry>,
    key: Zeroizing<Vec<u8>>,
    salt: Vec<u8>,
    params: KdfParams,
}

impl Drop for Vault {
    fn drop(&mut self) {
        // Entry secrets scrub themselves (Secret::drop); the key is Zeroizing.
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

    /// Decrypt the vault at `path` with `password`. A wrong password — or a
    /// tampered header — is reported as a clear "incorrect master password".
    pub fn unlock(path: &Path, password: &str) -> Result<Vault> {
        let data = fs::read_to_string(path).map_err(|e| anyhow!("cannot read vault file: {e}"))?;
        let file: VaultFile =
            serde_json::from_str(&data).map_err(|e| anyhow!("vault file is corrupt: {e}"))?;
        if file.version != VERSION {
            bail!("unsupported vault version {}", file.version);
        }
        // Validate KDF params from the untrusted file BEFORE deriving (stops the
        // allocation DoS and the downgrade-below-floor attack).
        file.kdf.validate()?;
        let salt = b64_decode(&file.salt)?;
        if salt.len() != SALT_LEN {
            bail!("vault file is corrupt: bad salt length");
        }
        let nonce = b64_decode(&file.nonce)?;
        if nonce.len() != NONCE_LEN {
            bail!("vault file is corrupt: bad nonce length");
        }
        let ciphertext = b64_decode(&file.ciphertext)?;
        let key = derive_key(password, &salt, &file.kdf)?;
        let aad = header_aad(file.version, &file.kdf, &salt);
        let plaintext = decrypt(&key, &nonce, &ciphertext, &aad)
            .map_err(|_| anyhow!("incorrect master password"))?;
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
        // Serialize a borrowing view so no second plaintext copy is made.
        let plaintext = Zeroizing::new(serde_json::to_vec(&PayloadRef {
            entries: &self.entries,
        })?);
        let nonce = random_bytes(NONCE_LEN)?;
        let aad = header_aad(VERSION, &self.params, &self.salt);
        let ciphertext = encrypt(&self.key, &nonce, &plaintext, &aad)?;
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

    /// Replace the entry at `editing` (if any), else append a new one. The
    /// overwritten entry's secret scrubs itself on drop (Secret).
    pub fn upsert(&mut self, editing: Option<usize>, entry: VaultEntry) {
        match editing {
            Some(i) if i < self.entries.len() => self.entries[i] = entry,
            _ => self.entries.push(entry),
        }
    }

    /// Remove the entry at `idx` (its secret scrubs itself on drop).
    pub fn remove(&mut self, idx: usize) {
        if idx < self.entries.len() {
            self.entries.remove(idx);
        }
    }
}

// ---------------------------------------------------------------------------
// Crypto primitives
// ---------------------------------------------------------------------------

/// The unencrypted header bound into the AEAD as associated data, so version,
/// KDF cost parameters and salt cannot be tampered with undetected.
fn header_aad(version: u32, p: &KdfParams, salt: &[u8]) -> Vec<u8> {
    let mut aad = Vec::with_capacity(16 + salt.len());
    aad.extend_from_slice(&version.to_be_bytes());
    aad.extend_from_slice(&p.m_cost.to_be_bytes());
    aad.extend_from_slice(&p.t_cost.to_be_bytes());
    aad.extend_from_slice(&p.p_cost.to_be_bytes());
    aad.extend_from_slice(salt);
    aad
}

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

fn encrypt(key: &[u8], nonce: &[u8], plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|e| anyhow!("cipher init: {e}"))?;
    cipher
        .encrypt(
            XNonce::from_slice(nonce),
            AeadPayload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|e| anyhow!("encryption failed: {e}"))
}

fn decrypt(key: &[u8], nonce: &[u8], ciphertext: &[u8], aad: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|e| anyhow!("cipher init: {e}"))?;
    let pt = cipher
        .decrypt(
            XNonce::from_slice(nonce),
            AeadPayload {
                msg: ciphertext,
                aad,
            },
        )
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

/// `path` with `.<ext>` appended (e.g. the `.bak` backup sidecar).
fn sidecar(path: &Path, ext: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".");
    s.push(ext);
    PathBuf::from(s)
}

/// A unique, unpredictable temp filename in the vault directory, so two
/// processes can't clobber each other and an attacker can't pre-create the path.
fn temp_name() -> Result<String> {
    let r = random_bytes(8)?;
    let hex: String = r.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!(".sshm-vault.{}.{hex}.tmp", std::process::id()))
}

/// Restrict a file to the current user only (no inheritance) on Windows, where
/// `0o600` does not apply. Best-effort: an icacls failure must never lose the
/// vault, so the result is intentionally ignored.
#[cfg(windows)]
fn restrict_acl(path: &Path) {
    let user = std::env::var("USERNAME").unwrap_or_default();
    if user.is_empty() {
        return;
    }
    let _ = std::process::Command::new("icacls")
        .arg(path)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(format!("{user}:(F)"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// Write `bytes` to `path` atomically and durably. The temp file is created
/// exclusively with a unique name, locked down to the owner (0o600 on unix,
/// owner-only ACL on Windows), then swapped in via a rename — moving the existing
/// vault to a `.bak` first so a crash mid-swap always leaves a recoverable copy
/// (the vault holds the only copy of irreplaceable secrets).
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    if !dir.exists() {
        fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
        }
    }

    let tmp = dir.join(temp_name()?);
    {
        use std::io::Write;
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        let _ = f.sync_all();
    }
    #[cfg(windows)]
    restrict_acl(&tmp);

    // Move the existing vault aside (works cross-platform; on Windows a rename
    // onto an existing destination fails, which is why we clear the path first).
    let bak = sidecar(path, "bak");
    let _ = fs::remove_file(&bak);
    if path.exists() {
        fs::rename(path, &bak)?;
    }
    match fs::rename(&tmp, path) {
        Ok(()) => {
            let _ = fs::remove_file(&bak);
            Ok(())
        }
        Err(e) => {
            // Restore the backup so we never end up with no vault at all.
            if bak.exists() {
                let _ = fs::rename(&bak, path);
            }
            let _ = fs::remove_file(&tmp);
            Err(e.into())
        }
    }
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
        assert_eq!(opened.entries[0].secret.as_str(), "s3cret");
        assert_eq!(opened.entries[1].kind, SecretKind::Passphrase);
        assert_eq!(opened.entries[1].secret.as_str(), "key-pass");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn non_ascii_secret_roundtrips() {
        let path = temp_vault_path("utf8");
        let mut v = Vault::create("マスター").unwrap();
        v.upsert(None, entry("h", SecretKind::Password, "pä55-🔐-é"));
        v.save(&path).unwrap();
        let opened = Vault::unlock(&path, "マスター").unwrap();
        assert_eq!(opened.entries[0].secret.as_str(), "pä55-🔐-é");
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
    fn unreasonable_kdf_params_are_rejected_without_allocating() {
        let path = temp_vault_path("kdf");
        let v = Vault::create("m").unwrap();
        v.save(&path).unwrap();

        // Tamper m_cost to a value that would allocate terabytes if used.
        let mut file: VaultFile =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        file.kdf.m_cost = u32::MAX;
        fs::write(&path, serde_json::to_string(&file).unwrap()).unwrap();

        match Vault::unlock(&path, "m") {
            Ok(_) => panic!("a vault with absurd KDF params should not unlock"),
            Err(e) => assert!(
                e.to_string().contains("KDF parameters"),
                "expected KDF rejection, got: {e}"
            ),
        }
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn header_tamper_is_detected() {
        let path = temp_vault_path("aad");
        let v = Vault::create("m").unwrap();
        v.save(&path).unwrap();

        // Flip t_cost within the valid range: it passes validation but no longer
        // matches the AEAD associated data, so the tag check must fail.
        let mut file: VaultFile =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        file.kdf.t_cost += 1;
        fs::write(&path, serde_json::to_string(&file).unwrap()).unwrap();

        assert!(
            Vault::unlock(&path, "m").is_err(),
            "tampered header must not unlock"
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn bad_salt_length_is_rejected() {
        let path = temp_vault_path("salt");
        let v = Vault::create("m").unwrap();
        v.save(&path).unwrap();
        let mut file: VaultFile =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        file.salt = B64.encode([0u8; 4]); // too short
        fs::write(&path, serde_json::to_string(&file).unwrap()).unwrap();
        match Vault::unlock(&path, "m") {
            Ok(_) => panic!("short salt should be rejected"),
            Err(e) => assert!(e.to_string().contains("salt"), "unexpected error: {e}"),
        }
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn upsert_replaces_in_place_and_remove_drops() {
        let mut v = Vault::create("m").unwrap();
        v.upsert(None, entry("a", SecretKind::Password, "1"));
        v.upsert(None, entry("b", SecretKind::Password, "2"));
        v.upsert(Some(0), entry("a", SecretKind::Password, "updated"));
        assert_eq!(v.entries[0].secret.as_str(), "updated");
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

    #[test]
    fn secret_debug_is_redacted() {
        let s: Secret = "topsecret".into();
        assert_eq!(format!("{s:?}"), "\"***\"");
        assert!(!format!("{s:?}").contains("topsecret"));
    }
}
