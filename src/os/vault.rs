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

/// Reject a secret that cannot be delivered over OpenSSH's line-oriented askpass
/// channel: it must not contain `\r`/`\n` (OpenSSH truncates at the first one)
/// and must fit OpenSSH's 1023-byte read cap. Enforced at vault-entry save time
/// so an unservable secret never reaches the connect-time auto-fill channel.
pub fn reject_unservable_secret(secret: &str) -> Result<()> {
    if secret.contains(['\r', '\n']) {
        return Err(anyhow!(
            "secret must not contain a newline or carriage return"
        ));
    }
    if secret.len() > 1023 {
        return Err(anyhow!("secret is too long (max 1023 bytes)"));
    }
    Ok(())
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

/// Which secret kinds a host has stored, for the auto-fill candidacy predicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MatchedKinds {
    pub password: bool,
    pub passphrase: bool,
}

impl MatchedKinds {
    pub fn any(self) -> bool {
        self.password || self.passphrase
    }
}

/// The pure auto-fill candidacy match: a host (given its `Host`-line `patterns`)
/// is a candidate if a vault entry's `host` equals ANY non-glob, non-negation
/// pattern. Returns the kinds present, or `None` if nothing matches. This is the
/// host<->entry match only — it is NOT the listener's prompt/identity-binding
/// release logic (see `os/askpass.rs`).
pub fn match_vault_kinds(patterns: &[String], entries: &[VaultEntry]) -> Option<MatchedKinds> {
    let mut m = MatchedKinds::default();
    for pat in patterns {
        if pat.contains(['*', '?', '!']) {
            continue;
        }
        for e in entries {
            if e.host == *pat {
                match e.kind {
                    SecretKind::Password => m.password = true,
                    SecretKind::Passphrase => m.passphrase = true,
                }
            }
        }
    }
    m.any().then_some(m)
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
        recover_backup(path)?;
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

    /// All stored secrets whose `host` field equals `host` (case-sensitive — the
    /// verbatim ssh destination). This is the connect-time *candidacy* lookup;
    /// the listener's identity binding (`os/askpass.rs`) decides actual release.
    pub fn secrets_for_host(&self, host: &str) -> Vec<&VaultEntry> {
        self.entries.iter().filter(|e| e.host == host).collect()
    }

    /// Remove the entry at `idx`, returning it so the caller can roll the removal
    /// back if a later step fails. Out-of-range is a no-op returning `None`. A
    /// discarded entry scrubs its secret on drop (Secret).
    pub fn remove(&mut self, idx: usize) -> Option<VaultEntry> {
        (idx < self.entries.len()).then(|| self.entries.remove(idx))
    }

    /// Upsert an entry **and persist**, keeping the in-memory vault in lock-step
    /// with disk: if the save fails the in-memory change is rolled back, so the
    /// list the user sees never diverges from what is stored. (A failed save
    /// leaves the previous file intact — see [`atomic_write`] — so rolling memory
    /// back to match is exactly right.) On the edit path the prior entry is briefly
    /// cloned, retained only to restore it on failure; it zeroizes on drop like
    /// every other [`Secret`].
    pub fn upsert_and_save(
        &mut self,
        editing: Option<usize>,
        entry: VaultEntry,
        path: &Path,
    ) -> Result<()> {
        let prev = match editing {
            Some(i) if i < self.entries.len() => Some((i, self.entries[i].clone())),
            _ => None,
        };
        self.upsert(editing, entry);
        if let Err(e) = self.save(path) {
            match prev {
                Some((i, old)) => self.entries[i] = old, // restore the replaced entry
                None => {
                    self.entries.pop(); // undo the append
                }
            }
            return Err(e);
        }
        Ok(())
    }

    /// Remove an entry **and persist**, rolling the removal back into memory if
    /// the save fails (the failed save left the file intact), so memory matches
    /// disk. A no-op for an out-of-range index.
    pub fn remove_and_save(&mut self, idx: usize, path: &Path) -> Result<()> {
        let Some(removed) = self.remove(idx) else {
            return Ok(()); // nothing removed; disk already matches memory
        };
        if let Err(e) = self.save(path) {
            self.entries.insert(idx, removed); // roll the removal back
            return Err(e);
        }
        Ok(())
    }

    /// Verify `password` against the unlocked vault by re-deriving the key with the
    /// current salt+params and comparing it (in constant time) to the in-memory
    /// key. True iff `password` is the current master password. Authorizes a
    /// [`Vault::rekey`]: an unlocked-but-idle session must not let a walk-up
    /// attacker change the master password without proving the old one.
    pub fn verify_password(&self, password: &str) -> bool {
        match derive_key(password, &self.salt, &self.params) {
            Ok(candidate) => ct_eq(&candidate, &self.key),
            Err(_) => false,
        }
    }

    /// True iff the vault's KDF parameters are **strictly weaker** than the current
    /// [`KdfParams::default`] — every field `<=` default and at least one `<` it
    /// (dominated by the default). A vault equal to, stronger than, or *mixed*
    /// (stronger in any field) against the default returns false, so a manually
    /// strengthened vault is never nudged into a downgrade. Gates the KDF-upgrade
    /// affordance in the UI (a rekey to the default would never lower a field).
    pub fn needs_kdf_upgrade(&self) -> bool {
        let d = KdfParams::default();
        let weakly_dominated = self.params.m_cost <= d.m_cost
            && self.params.t_cost <= d.t_cost
            && self.params.p_cost <= d.p_cost;
        let strictly_weaker = self.params.m_cost < d.m_cost
            || self.params.t_cost < d.t_cost
            || self.params.p_cost < d.p_cost;
        weakly_dominated && strictly_weaker
    }

    /// Re-encrypt the vault under `new_password` with a **fresh salt** and the
    /// current [`KdfParams::default`], then persist to `path`. Changing the master
    /// password and upgrading an old vault's KDF are the same operation — a
    /// KDF-only upgrade passes the *current* password as `new_password`.
    ///
    /// The caller must have authorized the change (see [`Vault::verify_password`]).
    /// On a save failure the previous `(key, salt, params)` are restored so the
    /// in-memory vault matches the untouched on-disk file — the same rollback
    /// discipline as [`Vault::upsert_and_save`]. Skipping it would leave the
    /// in-memory key diverged from disk, so the next entry save would re-encrypt
    /// under a key the file was never written with and lock the user out.
    pub fn rekey(&mut self, new_password: &str, path: &Path) -> Result<()> {
        // Field-wise max of the vault's params and the current default: a weak
        // vault is raised to default (the KDF upgrade), while a hand-hardened or
        // cross-version vault stronger in any field is never downgraded — so a
        // plain password change can only strengthen or hold the KDF, never weaken
        // it (honoring the same "no downgrade" invariant as `needs_kdf_upgrade`).
        let d = KdfParams::default();
        let new_params = KdfParams {
            m_cost: self.params.m_cost.max(d.m_cost),
            t_cost: self.params.t_cost.max(d.t_cost),
            p_cost: self.params.p_cost.max(d.p_cost),
        };
        let new_salt = random_bytes(SALT_LEN)?;
        let new_key = derive_key(new_password, &new_salt, &new_params)?;
        // Swap the new material in, keeping the old for rollback. `mem::replace`
        // hands back ownership, so on success the old key (Zeroizing) scrubs on
        // drop at function end; the salt is public so needs no scrubbing.
        let old_key = std::mem::replace(&mut self.key, new_key);
        let old_salt = std::mem::replace(&mut self.salt, new_salt);
        let old_params = std::mem::replace(&mut self.params, new_params);
        if let Err(e) = self.save(path) {
            self.key = old_key;
            self.salt = old_salt;
            self.params = old_params;
            return Err(e);
        }
        Ok(())
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

/// Constant-time byte-slice equality, used to compare a candidate derived key
/// against the in-memory key so verifying a master password leaks no timing
/// signal about how many bytes matched. Unequal lengths are unequal (both are
/// `KEY_LEN`); equal lengths are compared without early exit.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

pub(crate) fn random_bytes(n: usize) -> Result<Vec<u8>> {
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

/// Recover from a save that crashed mid-swap: if the vault file is missing but
/// its `.bak` backup survived, restore the backup. Call it before deciding
/// whether a vault exists (so a survivor isn't mistaken for "no vault" and
/// clobbered by a fresh create) and before reading the vault. A restore failure
/// is surfaced (NOT swallowed) so the caller never silently treats an existing,
/// un-restored backup as "no vault".
pub fn recover_backup(path: &Path) -> Result<()> {
    let bak = sidecar(path, "bak");
    if !path.exists() && bak.exists() {
        fs::rename(&bak, path).map_err(|e| {
            anyhow!(
                "a vault backup exists but could not be restored ({e}); your secrets are safe in {}",
                bak.display()
            )
        })?;
    }
    Ok(())
}

/// Write `bytes` to `path` atomically and durably. The temp file is created
/// exclusively with a unique name, locked down to the owner (0o600 on unix,
/// owner-only ACL on Windows), fsynced, then swapped in via a rename — moving the
/// existing vault to a `.bak` first so a crash mid-swap always leaves a recoverable
/// copy (the vault holds the only copy of irreplaceable secrets). The temp file is
/// removed on every failure path so a failed save never leaks an encrypted orphan.
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    // Owner-private dir on BOTH platforms (closes the Windows dir-ACL gap, #7).
    crate::secure_fs::create_dir_private(dir)?;

    let tmp = dir.join(crate::secure_fs::temp_name(".sshm-vault")?);
    // create_new_private creates O_EXCL and, on Windows, applies the owner-only
    // ACL BEFORE any ciphertext is written into the temp (#7); on unix it is
    // 0o600 from birth. icacls is resolved to an absolute path (#8). On ANY
    // failure remove the temp so a failed save never leaks an encrypted orphan,
    // and propagate the fsync result so a flush failure (EIO/ENOSPC) aborts
    // BEFORE the destructive swap.
    let write_res = (|| -> Result<()> {
        use std::io::Write;
        let mut f = crate::secure_fs::create_new_private(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok(())
    })();
    if let Err(e) = write_res {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }

    let bak = sidecar(path, "bak");
    if path.exists() {
        // A real vault is at `path`, so any leftover `.bak` is stale and safe to
        // drop. Back it up, swap in the temp, then drop the backup — at every
        // crash point either `path` or `.bak` holds a complete vault.
        let _ = fs::remove_file(&bak);
        if let Err(e) = fs::rename(path, &bak) {
            let _ = fs::remove_file(&tmp);
            return Err(e.into());
        }
        match fs::rename(&tmp, path) {
            Ok(()) => {
                let _ = fs::remove_file(&bak);
                crate::secure_fs::fsync_parent_dir(dir);
                Ok(())
            }
            Err(e) => {
                // Restore the backup so we never end up with no vault at all.
                if !path.exists() && bak.exists() {
                    let _ = fs::rename(&bak, path);
                }
                let _ = fs::remove_file(&tmp);
                Err(e.into())
            }
        }
    } else {
        // No vault at `path`. NEVER touch a surviving `.bak` here — it may be a
        // crash-orphaned real vault that recovery (not this create) must restore.
        // Deleting it would destroy the only copy of the user's secrets.
        if let Err(e) = fs::rename(&tmp, path) {
            let _ = fs::remove_file(&tmp);
            return Err(e.into());
        }
        crate::secure_fs::fsync_parent_dir(dir);
        Ok(())
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
    fn match_vault_kinds_basics() {
        let entries = vec![
            entry("web1", SecretKind::Password, "p"),
            entry("web1", SecretKind::Passphrase, "k"),
            entry("db", SecretKind::Password, "p"),
        ];
        // exact alias match, both kinds.
        assert_eq!(
            match_vault_kinds(&["web1".to_string()], &entries),
            Some(MatchedKinds {
                password: true,
                passphrase: true
            })
        );
        // a second pattern on the Host line matches too.
        assert_eq!(
            match_vault_kinds(&["nope".to_string(), "db".to_string()], &entries),
            Some(MatchedKinds {
                password: true,
                passphrase: false
            })
        );
        // no match -> None.
        assert_eq!(match_vault_kinds(&["other".to_string()], &entries), None);
        // glob / negation patterns are never candidates.
        assert_eq!(match_vault_kinds(&["web*".to_string()], &entries), None);
        assert_eq!(match_vault_kinds(&["!web1".to_string()], &entries), None);
    }

    #[test]
    fn secrets_for_host_returns_matching_kinds() {
        let mut v = Vault::create("pw").unwrap();
        v.upsert(None, entry("web1", SecretKind::Password, "p"));
        v.upsert(None, entry("web1", SecretKind::Passphrase, "k"));
        v.upsert(None, entry("db", SecretKind::Password, "d"));
        let got = v.secrets_for_host("web1");
        assert_eq!(got.len(), 2);
        assert!(
            got.iter()
                .any(|e| e.kind == SecretKind::Password && e.secret.as_str() == "p")
        );
        assert!(got.iter().any(|e| e.kind == SecretKind::Passphrase));
        assert!(v.secrets_for_host("nope").is_empty());
    }

    #[test]
    fn secret_validation_rejects_newlines() {
        assert!(reject_unservable_secret("ok").is_ok());
        assert!(reject_unservable_secret("two\nlines").is_err());
        assert!(reject_unservable_secret("cr\rret").is_err());
        assert!(reject_unservable_secret(&"x".repeat(1024)).is_err()); // > OpenSSH 1023 cap
        assert!(reject_unservable_secret(&"x".repeat(1023)).is_ok());
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
    fn save_draws_a_fresh_nonce_each_time() {
        // XChaCha20-Poly1305 nonce reuse under the fixed per-vault key would be
        // catastrophic (it breaks confidentiality + authenticity). Guard that two
        // saves of the same in-memory vault emit a different nonce (and therefore a
        // different ciphertext), so a future refactor that hoisted/cached the nonce
        // can't slip through.
        let mut v = Vault::create("m").unwrap();
        v.upsert(None, entry("web1", SecretKind::Password, "p"));
        let a = temp_vault_path("nonce-a");
        let b = temp_vault_path("nonce-b");
        v.save(&a).unwrap();
        v.save(&b).unwrap();

        let fa: VaultFile = serde_json::from_str(&fs::read_to_string(&a).unwrap()).unwrap();
        let fb: VaultFile = serde_json::from_str(&fs::read_to_string(&b).unwrap()).unwrap();
        assert_ne!(fa.nonce, fb.nonce, "each save must draw a fresh nonce");
        assert_ne!(
            fa.ciphertext, fb.ciphertext,
            "a fresh nonce must change the ciphertext"
        );

        let _ = fs::remove_file(&a);
        let _ = fs::remove_file(&b);
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
    fn recovers_from_orphaned_backup() {
        let path = temp_vault_path("recover");
        let mut v = Vault::create("m").unwrap();
        v.upsert(None, entry("h", SecretKind::Password, "s"));
        v.save(&path).unwrap();
        // Simulate a save that crashed mid-swap: main file gone, only .bak left.
        let bak = sidecar(&path, "bak");
        fs::rename(&path, &bak).unwrap();
        assert!(!path.exists() && bak.exists());
        // unlock transparently restores the backup and opens it.
        let opened = Vault::unlock(&path, "m").unwrap();
        assert_eq!(opened.entries[0].secret.as_str(), "s");
        assert!(path.exists() && !bak.exists());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn create_save_never_deletes_a_surviving_backup() {
        let path = temp_vault_path("guard");
        let bak = sidecar(&path, "bak");
        // A real vault survives only as `.bak` (a save crashed mid-swap).
        let mut real = Vault::create("m").unwrap();
        real.upsert(None, entry("h", SecretKind::Password, "keepme"));
        real.save(&path).unwrap();
        fs::rename(&path, &bak).unwrap(); // path absent, .bak holds the real vault

        // A fresh create save (e.g. if the user were wrongly dropped into create
        // mode) must NOT delete the surviving backup — the secrets stay safe.
        Vault::create("other").unwrap().save(&path).unwrap();
        assert!(
            bak.exists(),
            "a surviving .bak must never be deleted by a create-mode save"
        );
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(&bak);
    }

    #[test]
    fn txn_save_keeps_memory_and_disk_in_sync() {
        let path = temp_vault_path("txn");
        let mut v = Vault::create("m").unwrap();
        v.upsert_and_save(None, entry("a", SecretKind::Password, "1"), &path)
            .unwrap();
        v.upsert_and_save(None, entry("b", SecretKind::Password, "2"), &path)
            .unwrap();
        v.upsert_and_save(Some(0), entry("a", SecretKind::Password, "1b"), &path)
            .unwrap();
        v.remove_and_save(1, &path).unwrap();
        // In memory: a single edited entry remains.
        assert_eq!(v.entries.len(), 1);
        assert_eq!(v.entries[0].host, "a");
        assert_eq!(v.entries[0].secret.as_str(), "1b");
        // On disk: exactly what is in memory.
        let disk = Vault::unlock(&path, "m").unwrap();
        assert_eq!(disk.entries.len(), 1);
        assert_eq!(disk.entries[0].host, "a");
        assert_eq!(disk.entries[0].secret.as_str(), "1b");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_failed_save_rolls_back_the_in_memory_mutation() {
        // A vault path whose parent is a regular FILE makes every save fail
        // deterministically and cross-platform (you cannot create the temp file
        // inside a file), so we can prove the rollback without mocking I/O.
        let blocker = temp_vault_path("rollback-blocker");
        fs::write(&blocker, b"x").unwrap();
        let bad_path = blocker.join("vault.json");

        let mut v = Vault::create("m").unwrap();
        v.upsert(None, entry("a", SecretKind::Password, "1"));
        v.upsert(None, entry("b", SecretKind::Password, "2"));
        let baseline = 2;

        // A failed ADD must not leave the appended entry behind.
        assert!(
            v.upsert_and_save(None, entry("c", SecretKind::Password, "3"), &bad_path)
                .is_err()
        );
        assert_eq!(v.entries.len(), baseline, "failed add must roll back");
        assert!(v.entries.iter().all(|e| e.host != "c"));

        // A failed EDIT must restore the prior entry value.
        assert!(
            v.upsert_and_save(
                Some(0),
                entry("a", SecretKind::Password, "EDITED"),
                &bad_path
            )
            .is_err()
        );
        assert_eq!(v.entries.len(), baseline);
        assert_eq!(
            v.entries[0].secret.as_str(),
            "1",
            "failed edit must roll back to the old value"
        );

        // A failed REMOVE must restore the entry at its original index.
        assert!(v.remove_and_save(0, &bad_path).is_err());
        assert_eq!(v.entries.len(), baseline, "failed remove must roll back");
        assert_eq!(v.entries[0].host, "a");
        assert_eq!(v.entries[1].host, "b");

        let _ = fs::remove_file(&blocker);
    }

    #[test]
    fn secret_debug_is_redacted() {
        let s: Secret = "topsecret".into();
        assert_eq!(format!("{s:?}"), "\"***\"");
        assert!(!format!("{s:?}").contains("topsecret"));
    }

    #[test]
    fn verify_password_accepts_correct_rejects_wrong() {
        // given a vault protected by a known master password
        let v = Vault::create("correct horse").unwrap();
        // when / then: only the exact master password verifies (constant-time re-derive)
        assert!(v.verify_password("correct horse"));
        assert!(!v.verify_password("wrong"));
        assert!(!v.verify_password(""));
    }

    #[test]
    fn needs_kdf_upgrade_false_for_default_params() {
        // given a freshly created vault (which uses KdfParams::default())
        let v = Vault::create("m").unwrap();
        // then no upgrade is offered — the current params are exactly the default
        assert!(!v.needs_kdf_upgrade());
    }

    #[test]
    fn needs_kdf_upgrade_true_when_strictly_weaker() {
        // given a vault whose m_cost was weakened below the default (t_cost/p_cost
        // left at default), so every field is <= default and one is strictly <
        let mut v = Vault::create("m").unwrap();
        v.params.m_cost = MIN_M_COST; // below the default 19_456
        // then it is strictly dominated by default and an upgrade is offered
        assert!(v.needs_kdf_upgrade());
    }

    #[test]
    fn needs_kdf_upgrade_false_when_stronger_or_mixed() {
        // given a vault strictly STRONGER than default (higher m_cost)
        let mut stronger = Vault::create("m").unwrap();
        stronger.params.m_cost = KdfParams::default().m_cost + 10_000; // still <= MAX_M_COST
        // then a manually-strengthened vault is never offered a downgrade
        assert!(!stronger.needs_kdf_upgrade());

        // given a MIXED vault (weaker m_cost but STRONGER t_cost) — not dominated
        let mut mixed = Vault::create("m").unwrap();
        mixed.params.m_cost = MIN_M_COST;
        mixed.params.t_cost = KdfParams::default().t_cost + 1;
        // then it is not offered a downgrade either (stronger in one field)
        assert!(!mixed.needs_kdf_upgrade());
    }

    #[test]
    fn rekey_changes_the_master_password() {
        // given a saved vault protected by "old-pass" with a Password and a Passphrase
        let path = temp_vault_path("rekey-change");
        let mut v = Vault::create("old-pass").unwrap();
        v.upsert(None, entry("web", SecretKind::Password, "s3cret"));
        v.upsert(None, entry("web", SecretKind::Passphrase, "key-pass"));
        v.save(&path).unwrap();

        // when the master password is changed to "new-pass"
        v.rekey("new-pass", &path).unwrap();

        // then the new password unlocks the file and the entries are preserved
        let opened = Vault::unlock(&path, "new-pass").unwrap();
        assert_eq!(opened.entries.len(), 2);
        assert_eq!(opened.entries[0].host, "web");
        assert_eq!(opened.entries[0].kind, SecretKind::Password);
        assert_eq!(opened.entries[0].secret.as_str(), "s3cret");
        assert_eq!(opened.entries[1].kind, SecretKind::Passphrase);
        assert_eq!(opened.entries[1].secret.as_str(), "key-pass");

        // and the old password no longer works
        match Vault::unlock(&path, "old-pass") {
            Ok(_) => panic!("old password must not unlock after a rekey"),
            Err(e) => assert!(
                e.to_string().contains("incorrect master password"),
                "unexpected error: {e}"
            ),
        }

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn rekey_upgrades_kdf_params_and_rotates_salt() {
        // given a saved vault whose KDF was weakened below default
        let path = temp_vault_path("rekey-kdf");
        let mut v = Vault::create("m").unwrap();
        v.params.m_cost = MIN_M_COST;
        v.save(&path).unwrap();

        // capture the on-disk salt + kdf before the rekey
        let before: VaultFile = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let old_salt = before.salt.clone();
        assert_eq!(before.kdf.m_cost, MIN_M_COST); // sanity: the weakened value was written

        // when rekeyed
        v.rekey("whatever", &path).unwrap();

        // then the on-disk KDF params are re-keyed back to default and the salt rotated
        let after: VaultFile = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(after.kdf.m_cost, KdfParams::default().m_cost);
        assert_eq!(after.kdf.t_cost, KdfParams::default().t_cost);
        assert_eq!(after.kdf.p_cost, KdfParams::default().p_cost);
        assert_ne!(after.salt, old_salt, "rekey must rotate to a fresh salt");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn rekey_rolls_back_on_save_failure() {
        // A vault path whose parent is a regular FILE makes every save fail
        // deterministically and cross-platform (same technique as
        // a_failed_save_rolls_back_the_in_memory_mutation), so the rollback is
        // provable without mocking I/O.
        let blocker = temp_vault_path("rekey-rollback-blocker");
        fs::write(&blocker, b"x").unwrap();
        let bad_path = blocker.join("vault.json");

        // given an in-memory vault protected by "old-pass"
        let mut v = Vault::create("old-pass").unwrap();
        v.upsert(None, entry("h", SecretKind::Password, "s"));

        // when a rekey save fails
        assert!(v.rekey("new-pass", &bad_path).is_err());

        // then the in-memory (key, salt, params) were rolled back so memory matches
        // the unchanged on-disk file: the OLD password verifies, the NEW one does not.
        assert!(
            v.verify_password("old-pass"),
            "a failed rekey must roll the key back to the old password"
        );
        assert!(
            !v.verify_password("new-pass"),
            "the new password must not take effect when the save failed"
        );

        let _ = fs::remove_file(&blocker);
    }

    #[test]
    fn rekey_never_downgrades_stronger_or_mixed_params() {
        // A password change must strengthen-or-hold the KDF, never lower a field:
        // otherwise changing the master password would silently downgrade a
        // hand-hardened or cross-version vault (the very invariant
        // `needs_kdf_upgrade` protects). rekey re-keys with the field-wise max of
        // the vault's params and the current default.
        let d = KdfParams::default();

        // given a vault STRONGER than default in every field
        let path = temp_vault_path("rekey-no-downgrade-stronger");
        let mut v = Vault::create("pw").unwrap();
        v.params = KdfParams {
            m_cost: d.m_cost + 10_000,
            t_cost: d.t_cost + 1,
            p_cost: d.p_cost,
        };
        v.save(&path).unwrap();

        // when the master password is changed
        v.rekey("new-pw", &path).unwrap();

        // then no field was lowered (the stronger values are retained)
        let f: VaultFile = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            f.kdf.m_cost,
            d.m_cost + 10_000,
            "stronger m_cost must be held"
        );
        assert_eq!(f.kdf.t_cost, d.t_cost + 1, "stronger t_cost must be held");
        assert_eq!(f.kdf.p_cost, d.p_cost);
        let _ = fs::remove_file(&path);

        // and a MIXED vault (weaker m_cost, stronger t_cost): the weak field is
        // raised to default while the strong field is NOT downgraded.
        let path2 = temp_vault_path("rekey-no-downgrade-mixed");
        let mut v2 = Vault::create("pw").unwrap();
        v2.params = KdfParams {
            m_cost: MIN_M_COST,
            t_cost: d.t_cost + 1,
            p_cost: d.p_cost,
        };
        v2.save(&path2).unwrap();
        v2.rekey("new-pw", &path2).unwrap();
        let f2: VaultFile = serde_json::from_str(&fs::read_to_string(&path2).unwrap()).unwrap();
        assert_eq!(
            f2.kdf.m_cost, d.m_cost,
            "weak m_cost must be upgraded to default"
        );
        assert_eq!(
            f2.kdf.t_cost,
            d.t_cost + 1,
            "stronger t_cost must be held (no downgrade)"
        );
        let _ = fs::remove_file(&path2);
    }
}
