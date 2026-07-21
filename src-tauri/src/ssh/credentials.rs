use crate::ssh::session::config_dir;
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use zeroize::{Zeroize, Zeroizing};

const VAULT_VERSION: u8 = 1;
const VAULT_CIPHER: &str = "AES-256-GCM";
const VAULT_KDF: &str = "Argon2id";
const VAULT_MAGIC: &str = "GpuTerm credential vault";
const VAULT_AAD: &[u8] = b"GpuTerm credential vault v1";
const SALT_BYTES: usize = 16;
const NONCE_BYTES: usize = 12;
const KEY_BYTES: usize = 32;
const KDF_MEMORY_KIB: u32 = 64 * 1024;
const KDF_ITERATIONS: u32 = 3;
const KDF_PARALLELISM: u32 = 1;
const MIN_MASTER_PASSWORD_CHARS: usize = 8;

pub trait CredentialStore: Send + Sync {
    fn set_password(&self, session_id: &str, password: String) -> Result<(), String>;
    fn get_password(&self, session_id: &str) -> Result<Option<String>, String>;
    fn clear_password(&self, session_id: &str) -> Result<(), String>;
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CredentialVaultStatus {
    pub exists: bool,
    pub unlocked: bool,
    pub has_credentials: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VaultEnvelope {
    version: u8,
    cipher: String,
    kdf: VaultKdf,
    nonce: String,
    ciphertext: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VaultKdf {
    algorithm: String,
    salt: String,
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
}

#[derive(Clone, Serialize, Deserialize)]
struct VaultPayload {
    magic: String,
    entries: BTreeMap<String, String>,
}

impl Default for VaultPayload {
    fn default() -> Self {
        Self {
            magic: VAULT_MAGIC.to_string(),
            entries: BTreeMap::new(),
        }
    }
}

impl Drop for VaultPayload {
    fn drop(&mut self) {
        self.magic.zeroize();
        for password in self.entries.values_mut() {
            password.zeroize();
        }
    }
}

#[derive(Default)]
struct VaultRuntime {
    key: Option<Zeroizing<[u8; KEY_BYTES]>>,
    payload: Option<VaultPayload>,
    known_present: HashSet<String>,
}

#[derive(Clone)]
pub struct SecureCredentialStore {
    runtime: Arc<Mutex<VaultRuntime>>,
    vault_path: Arc<PathBuf>,
    index_path: Arc<PathBuf>,
}

impl Default for SecureCredentialStore {
    fn default() -> Self {
        let base = config_dir();
        Self::with_paths(
            base.join("credentials.enc"),
            base.join("credential_index.json"),
        )
    }
}

impl SecureCredentialStore {
    fn with_paths(vault_path: PathBuf, index_path: PathBuf) -> Self {
        let known_present = read_credential_index(&index_path);
        Self {
            runtime: Arc::new(Mutex::new(VaultRuntime {
                known_present,
                ..VaultRuntime::default()
            })),
            vault_path: Arc::new(vault_path),
            index_path: Arc::new(index_path),
        }
    }

    pub fn status(&self) -> CredentialVaultStatus {
        let exists = self.vault_path.exists();
        self.runtime
            .lock()
            .map(|runtime| CredentialVaultStatus {
                exists,
                unlocked: runtime.key.is_some() && runtime.payload.is_some(),
                has_credentials: exists && !runtime.known_present.is_empty(),
            })
            .unwrap_or(CredentialVaultStatus {
                exists,
                unlocked: false,
                has_credentials: false,
            })
    }

    pub fn initialize(&self, master_password: String) -> Result<CredentialVaultStatus, String> {
        let master_password = Zeroizing::new(master_password);
        validate_new_master_password(&master_password)?;
        let mut runtime = self
            .runtime
            .lock()
            .map_err(|_| "Credential vault state is unavailable".to_string())?;
        if self.vault_path.exists() {
            return Err("Credential vault already exists; unlock it instead".to_string());
        }

        let mut salt = [0_u8; SALT_BYTES];
        OsRng.fill_bytes(&mut salt);
        let kdf = VaultKdf {
            algorithm: VAULT_KDF.to_string(),
            salt: BASE64.encode(salt),
            memory_kib: KDF_MEMORY_KIB,
            iterations: KDF_ITERATIONS,
            parallelism: KDF_PARALLELISM,
        };
        let key = derive_key(master_password.as_bytes(), &kdf)?;
        let payload = VaultPayload::default();
        write_encrypted_vault(&self.vault_path, &key, &kdf, &payload)?;

        runtime.known_present.clear();
        self.sync_index(&runtime);
        runtime.key = Some(key);
        runtime.payload = Some(payload);
        Ok(CredentialVaultStatus {
            exists: true,
            unlocked: true,
            has_credentials: false,
        })
    }

    pub fn unlock(&self, master_password: String) -> Result<CredentialVaultStatus, String> {
        let master_password = Zeroizing::new(master_password);
        let envelope = read_vault_envelope(&self.vault_path)?;
        validate_envelope(&envelope)?;
        let key = derive_key(master_password.as_bytes(), &envelope.kdf)?;
        let payload = decrypt_payload(&key, &envelope).map_err(|_| {
            "Incorrect master password or the credential vault is corrupted".to_string()
        })?;
        if payload.magic != VAULT_MAGIC {
            return Err(
                "Incorrect master password or the credential vault is corrupted".to_string(),
            );
        }

        let mut runtime = self
            .runtime
            .lock()
            .map_err(|_| "Credential vault state is unavailable".to_string())?;
        runtime.known_present = payload.entries.keys().cloned().collect();
        self.sync_index(&runtime);
        let has_credentials = !runtime.known_present.is_empty();
        runtime.key = Some(key);
        runtime.payload = Some(payload);
        Ok(CredentialVaultStatus {
            exists: true,
            unlocked: true,
            has_credentials,
        })
    }

    pub fn reset(&self) -> Result<CredentialVaultStatus, String> {
        let mut runtime = self
            .runtime
            .lock()
            .map_err(|_| "Credential vault state is unavailable".to_string())?;
        runtime.key = None;
        runtime.payload = None;
        runtime.known_present.clear();
        remove_file_if_present(&self.vault_path, "credential vault")?;
        remove_file_if_present(&self.index_path, "credential metadata")?;
        Ok(CredentialVaultStatus {
            exists: false,
            unlocked: false,
            has_credentials: false,
        })
    }

    /// Returns only non-secret UI metadata and never derives a key or decrypts
    /// the vault. The index is reconciled from authenticated data on unlock.
    pub fn has_saved_credential(&self, session_id: &str) -> bool {
        self.runtime
            .lock()
            .map(|runtime| runtime.known_present.contains(session_id))
            .unwrap_or(false)
    }

    fn persist_runtime(runtime: &VaultRuntime, path: &Path) -> Result<(), String> {
        let key = runtime.key.as_ref().ok_or_else(vault_locked_error)?;
        let payload = runtime.payload.as_ref().ok_or_else(vault_locked_error)?;
        let envelope = read_vault_envelope(path)?;
        validate_envelope(&envelope)?;
        write_encrypted_vault(path, key, &envelope.kdf, payload)
    }

    fn sync_index(&self, runtime: &VaultRuntime) {
        // The index contains no secrets and is only a pre-unlock UI hint. The
        // authenticated payload remains authoritative, so an index write must
        // never make an otherwise successful vault operation look like it failed.
        let _ = write_credential_index(&self.index_path, &runtime.known_present);
    }
}

impl CredentialStore for SecureCredentialStore {
    fn set_password(&self, session_id: &str, password: String) -> Result<(), String> {
        let password = Zeroizing::new(password);
        let mut runtime = self
            .runtime
            .lock()
            .map_err(|_| "Credential vault state is unavailable".to_string())?;
        let previous = {
            let payload = runtime.payload.as_mut().ok_or_else(vault_locked_error)?;
            if payload.entries.get(session_id).map(String::as_str) == Some(password.as_str()) {
                return Ok(());
            }
            payload
                .entries
                .insert(session_id.to_string(), password.to_string())
        };
        if let Err(error) = Self::persist_runtime(&runtime, &self.vault_path) {
            if let Some(payload) = runtime.payload.as_mut() {
                if let Some(mut unpersisted) = payload.entries.remove(session_id) {
                    unpersisted.zeroize();
                }
                if let Some(previous) = previous {
                    payload.entries.insert(session_id.to_string(), previous);
                }
            }
            return Err(error);
        }
        if let Some(mut previous) = previous {
            previous.zeroize();
        }
        runtime.known_present.insert(session_id.to_string());
        self.sync_index(&runtime);
        Ok(())
    }

    fn get_password(&self, session_id: &str) -> Result<Option<String>, String> {
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| "Credential vault state is unavailable".to_string())?;
        let payload = runtime.payload.as_ref().ok_or_else(vault_locked_error)?;
        Ok(payload.entries.get(session_id).cloned())
    }

    fn clear_password(&self, session_id: &str) -> Result<(), String> {
        let mut runtime = self
            .runtime
            .lock()
            .map_err(|_| "Credential vault state is unavailable".to_string())?;
        let removed = runtime
            .payload
            .as_mut()
            .ok_or_else(vault_locked_error)?
            .entries
            .remove(session_id);
        let Some(mut removed) = removed else {
            runtime.known_present.remove(session_id);
            self.sync_index(&runtime);
            return Ok(());
        };
        if let Err(error) = Self::persist_runtime(&runtime, &self.vault_path) {
            if let Some(payload) = runtime.payload.as_mut() {
                payload.entries.insert(session_id.to_string(), removed);
            }
            return Err(error);
        }
        removed.zeroize();
        runtime.known_present.remove(session_id);
        self.sync_index(&runtime);
        Ok(())
    }
}

fn vault_locked_error() -> String {
    "Credential vault is locked; unlock it with the GpuTerm master password".to_string()
}

fn validate_new_master_password(password: &str) -> Result<(), String> {
    if password.chars().count() < MIN_MASTER_PASSWORD_CHARS {
        return Err(format!(
            "Master password must be at least {} characters",
            MIN_MASTER_PASSWORD_CHARS
        ));
    }
    Ok(())
}

fn validate_envelope(envelope: &VaultEnvelope) -> Result<(), String> {
    if envelope.version != VAULT_VERSION
        || envelope.cipher != VAULT_CIPHER
        || envelope.kdf.algorithm != VAULT_KDF
    {
        return Err("Unsupported credential vault format".to_string());
    }
    if envelope.kdf.memory_kib != KDF_MEMORY_KIB
        || envelope.kdf.iterations != KDF_ITERATIONS
        || envelope.kdf.parallelism != KDF_PARALLELISM
    {
        return Err("Credential vault KDF parameters are outside safe limits".to_string());
    }
    Ok(())
}

fn derive_key(password: &[u8], kdf: &VaultKdf) -> Result<Zeroizing<[u8; KEY_BYTES]>, String> {
    validate_envelope(&VaultEnvelope {
        version: VAULT_VERSION,
        cipher: VAULT_CIPHER.to_string(),
        kdf: kdf.clone(),
        nonce: String::new(),
        ciphertext: String::new(),
    })?;
    let salt = BASE64
        .decode(&kdf.salt)
        .map_err(|_| "Credential vault salt is invalid".to_string())?;
    if salt.len() != SALT_BYTES {
        return Err("Credential vault salt has an invalid length".to_string());
    }
    let params = Params::new(
        kdf.memory_kib,
        kdf.iterations,
        kdf.parallelism,
        Some(KEY_BYTES),
    )
    .map_err(|error| format!("Credential vault KDF parameters are invalid: {}", error))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = Zeroizing::new([0_u8; KEY_BYTES]);
    argon2
        .hash_password_into(password, &salt, key.as_mut())
        .map_err(|error| format!("Failed to derive credential vault key: {}", error))?;
    Ok(key)
}

fn write_encrypted_vault(
    path: &Path,
    key: &[u8; KEY_BYTES],
    kdf: &VaultKdf,
    payload: &VaultPayload,
) -> Result<(), String> {
    let plaintext = Zeroizing::new(
        serde_json::to_vec(payload)
            .map_err(|error| format!("Failed to serialize credential vault: {}", error))?,
    );
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| "Failed to initialize credential vault cipher".to_string())?;
    let mut nonce_bytes = [0_u8; NONCE_BYTES];
    OsRng.fill_bytes(&mut nonce_bytes);
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload {
                msg: plaintext.as_slice(),
                aad: VAULT_AAD,
            },
        )
        .map_err(|_| "Failed to encrypt credential vault".to_string())?;
    let envelope = VaultEnvelope {
        version: VAULT_VERSION,
        cipher: VAULT_CIPHER.to_string(),
        kdf: kdf.clone(),
        nonce: BASE64.encode(nonce_bytes),
        ciphertext: BASE64.encode(ciphertext),
    };
    write_json_file(path, &envelope, "credential vault")
}

fn decrypt_payload(
    key: &[u8; KEY_BYTES],
    envelope: &VaultEnvelope,
) -> Result<VaultPayload, String> {
    let nonce = BASE64
        .decode(&envelope.nonce)
        .map_err(|_| "Credential vault nonce is invalid".to_string())?;
    if nonce.len() != NONCE_BYTES {
        return Err("Credential vault nonce has an invalid length".to_string());
    }
    let ciphertext = BASE64
        .decode(&envelope.ciphertext)
        .map_err(|_| "Credential vault ciphertext is invalid".to_string())?;
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| "Failed to initialize credential vault cipher".to_string())?;
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &ciphertext,
                    aad: VAULT_AAD,
                },
            )
            .map_err(|_| "Credential vault authentication failed".to_string())?,
    );
    serde_json::from_slice(&plaintext)
        .map_err(|_| "Credential vault plaintext is invalid".to_string())
}

fn read_vault_envelope(path: &Path) -> Result<VaultEnvelope, String> {
    let content = fs::read_to_string(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            "Credential vault has not been created".to_string()
        } else {
            format!(
                "Failed to read credential vault {}: {}",
                path.display(),
                error
            )
        }
    })?;
    serde_json::from_str(&content)
        .map_err(|_| "Credential vault file is corrupted or invalid".to_string())
}

fn read_credential_index(path: &Path) -> HashSet<String> {
    fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str::<Vec<String>>(&content).ok())
        .unwrap_or_default()
        .into_iter()
        .collect()
}

fn write_credential_index(path: &Path, known: &HashSet<String>) -> Result<(), String> {
    let mut ids = known.iter().cloned().collect::<Vec<_>>();
    ids.sort();
    write_json_file(path, &ids, "credential metadata")
}

fn write_json_file(path: &Path, value: &impl Serialize, label: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Failed to create {} directory {}: {}",
                label,
                parent.display(),
                error
            )
        })?;
    }
    let content = serde_json::to_string_pretty(value)
        .map_err(|error| format!("Failed to serialize {}: {}", label, error))?;
    let temp_path = path.with_extension(format!(
        "{}.{}.tmp",
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("new"),
        uuid::Uuid::new_v4()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut temp_file = options.open(&temp_path).map_err(|error| {
        format!(
            "Failed to open temporary {} {}: {}",
            label,
            temp_path.display(),
            error
        )
    })?;
    temp_file.write_all(content.as_bytes()).map_err(|error| {
        let _ = fs::remove_file(&temp_path);
        format!(
            "Failed to write temporary {} {}: {}",
            label,
            temp_path.display(),
            error
        )
    })?;
    temp_file.sync_all().map_err(|error| {
        let _ = fs::remove_file(&temp_path);
        format!(
            "Failed to sync temporary {} {}: {}",
            label,
            temp_path.display(),
            error
        )
    })?;
    drop(temp_file);

    #[cfg(target_os = "windows")]
    if path.exists() {
        fs::remove_file(path).map_err(|error| {
            let _ = fs::remove_file(&temp_path);
            format!("Failed to replace {} {}: {}", label, path.display(), error)
        })?;
    }
    fs::rename(&temp_path, path).map_err(|error| {
        let _ = fs::remove_file(&temp_path);
        format!("Failed to finalize {} {}: {}", label, path.display(), error)
    })
}

fn remove_file_if_present(path: &Path, label: &str) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "Failed to remove {} {}: {}",
            label,
            path.display(),
            error
        )),
    }
}

#[cfg(test)]
#[derive(Default)]
pub struct MemoryCredentialStore {
    passwords: Mutex<BTreeMap<String, String>>,
}

#[cfg(test)]
impl CredentialStore for MemoryCredentialStore {
    fn set_password(&self, session_id: &str, password: String) -> Result<(), String> {
        self.passwords
            .lock()
            .map_err(|_| "Credential memory is unavailable".to_string())?
            .insert(session_id.to_string(), password);
        Ok(())
    }

    fn get_password(&self, session_id: &str) -> Result<Option<String>, String> {
        Ok(self
            .passwords
            .lock()
            .map_err(|_| "Credential memory is unavailable".to_string())?
            .get(session_id)
            .cloned())
    }

    fn clear_password(&self, session_id: &str) -> Result<(), String> {
        self.passwords
            .lock()
            .map_err(|_| "Credential memory is unavailable".to_string())?
            .remove(session_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> (SecureCredentialStore, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "gputerm-vault-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let vault = root.join("credentials.enc");
        let index = root.join("credential_index.json");
        (
            SecureCredentialStore::with_paths(vault.clone(), index.clone()),
            vault,
            index,
        )
    }

    fn cleanup(vault: &Path, index: &Path) {
        if let Some(root) = vault.parent() {
            let _ = fs::remove_file(vault);
            let _ = fs::remove_file(index);
            let _ = fs::remove_dir(root);
        }
    }

    #[test]
    fn encrypts_credentials_and_restores_them_after_unlock() {
        let (store, vault, index) = test_store();
        store
            .initialize("correct horse battery staple".to_string())
            .unwrap();
        store
            .set_password("session-a", "ssh-super-secret".to_string())
            .unwrap();

        let vault_text = fs::read_to_string(&vault).unwrap();
        assert!(!vault_text.contains("session-a"));
        assert!(!vault_text.contains("ssh-super-secret"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&vault).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let restarted = SecureCredentialStore::with_paths(vault.clone(), index.clone());
        assert!(!restarted.status().unlocked);
        restarted
            .unlock("correct horse battery staple".to_string())
            .unwrap();
        assert_eq!(
            restarted.get_password("session-a").unwrap().as_deref(),
            Some("ssh-super-secret")
        );
        cleanup(&vault, &index);
    }

    #[test]
    fn rejects_wrong_master_password_and_tampered_ciphertext() {
        let (store, vault, index) = test_store();
        store
            .initialize("correct horse battery staple".to_string())
            .unwrap();
        assert!(store.unlock("wrong password".to_string()).is_err());

        let mut envelope = read_vault_envelope(&vault).unwrap();
        let mut ciphertext = BASE64.decode(&envelope.ciphertext).unwrap();
        ciphertext[0] ^= 0x80;
        envelope.ciphertext = BASE64.encode(ciphertext);
        write_json_file(&vault, &envelope, "credential vault").unwrap();
        let restarted = SecureCredentialStore::with_paths(vault.clone(), index.clone());
        assert!(restarted
            .unlock("correct horse battery staple".to_string())
            .unwrap_err()
            .contains("corrupted"));
        cleanup(&vault, &index);
    }

    #[test]
    fn uses_a_fresh_nonce_for_every_vault_write() {
        let (store, vault, index) = test_store();
        store
            .initialize("correct horse battery staple".to_string())
            .unwrap();
        let first_nonce = read_vault_envelope(&vault).unwrap().nonce;
        store
            .set_password("session-a", "first-secret".to_string())
            .unwrap();
        let second_nonce = read_vault_envelope(&vault).unwrap().nonce;
        store
            .set_password("session-b", "second-secret".to_string())
            .unwrap();
        let third_nonce = read_vault_envelope(&vault).unwrap().nonce;

        assert_ne!(first_nonce, second_nonce);
        assert_ne!(second_nonce, third_nonce);
        cleanup(&vault, &index);
    }

    #[test]
    fn requires_unlock_and_deletes_credentials_from_encrypted_payload() {
        let (store, vault, index) = test_store();
        assert!(store
            .set_password("session-a", "secret".to_string())
            .unwrap_err()
            .contains("locked"));
        store
            .initialize("correct horse battery staple".to_string())
            .unwrap();
        store
            .set_password("session-a", "secret".to_string())
            .unwrap();
        store.clear_password("session-a").unwrap();
        assert_eq!(store.get_password("session-a").unwrap(), None);
        assert!(!store.has_saved_credential("session-a"));
        cleanup(&vault, &index);
    }

    #[test]
    fn rejects_short_master_passwords() {
        let (store, vault, index) = test_store();
        assert!(store.initialize("short".to_string()).is_err());
        assert!(!vault.exists());
        cleanup(&vault, &index);
    }

    #[test]
    fn rejects_modified_kdf_parameters_before_derivation() {
        let (store, vault, index) = test_store();
        store
            .initialize("correct horse battery staple".to_string())
            .unwrap();
        let mut envelope = read_vault_envelope(&vault).unwrap();
        envelope.kdf.memory_kib = KDF_MEMORY_KIB * 2;
        write_json_file(&vault, &envelope, "credential vault").unwrap();

        let restarted = SecureCredentialStore::with_paths(vault.clone(), index.clone());
        assert!(restarted
            .unlock("correct horse battery staple".to_string())
            .unwrap_err()
            .contains("safe limits"));
        cleanup(&vault, &index);
    }
}
