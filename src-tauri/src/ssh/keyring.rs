//! The key manager: SSH key pairs made on this computer independently of any
//! server, then installed on and used by any number of profiles.
//!
//! Like a profile's managed key, a private key lives only in the encrypted
//! credential vault (`sshkey:key:<id>`) and signs in from memory. Its public
//! half, name and the profiles using it are kept in `keys.json`, so assigning
//! a key changes no profile.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};
use uuid::Uuid;
use zeroize::Zeroizing;

use super::credentials::{write_json_file, CredentialStore, SecureCredentialStore};
use super::managed_key::{
    attach_managed_keys, find_profile, generate_kind, install, key_only, render_export,
    write_private_file, KeyKind, SecretKey,
};
use super::resource_details::resolve_remote_os;
use super::session::{
    config_dir, list_profiles, open_ssh_session_with, target_for_profile,
    verify_managed_key_session, AppState,
};
use super::system_monitor::now_epoch_seconds;
use super::terminal::sign_in_responder;

static KEYRING_LOCK: Mutex<()> = Mutex::new(());
const MAX_NAME_CHARS: usize = 64;

/// One key in the manager. Nothing here is secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredKey {
    pub id: String,
    pub name: String,
    pub kind: KeyKind,
    /// `<algorithm> <base64> <comment>`, ready for authorized_keys.
    pub public_key: String,
    /// `SHA256:...`
    pub fingerprint: String,
    pub created_at: u64,
    /// Profiles that sign in with this key. A profile uses at most one.
    #[serde(default)]
    pub profile_ids: Vec<String>,
}

/// The vault entry holding a key manager key.
pub fn key_vault_key(key_id: &str) -> String {
    format!("sshkey:key:{key_id}")
}

fn keys_path() -> PathBuf {
    config_dir().join("keys.json")
}

fn lock() -> Result<std::sync::MutexGuard<'static, ()>, String> {
    KEYRING_LOCK
        .lock()
        .map_err(|_| "The SSH key list is unavailable".to_string())
}

fn read_keys(path: &Path) -> Result<Vec<StoredKey>, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| format!("The SSH key list is unreadable: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(format!("Could not read the SSH key list: {error}")),
    }
}

fn write_keys(path: &Path, keys: &[StoredKey]) -> Result<(), String> {
    write_json_file(path, &keys, "SSH key list")
}

/// Reads, changes and writes the list under one lock.
fn update_keys(
    path: &Path,
    change: impl FnOnce(&mut Vec<StoredKey>) -> Result<(), String>,
) -> Result<Vec<StoredKey>, String> {
    let _guard = lock()?;
    let mut keys = read_keys(path)?;
    change(&mut keys)?;
    write_keys(path, &keys)?;
    Ok(keys)
}

/// The key a profile signs in with, if one is assigned. Read at every
/// sign-in, so a change applies to the next connection.
pub fn assigned_key_id(profile_id: &str) -> Option<String> {
    let _guard = lock().ok()?;
    let keys = read_keys(&keys_path()).ok()?;
    key_for_profile(&keys, profile_id).map(|key| key.id.clone())
}

pub(crate) fn key_for_profile<'a>(
    keys: &'a [StoredKey],
    profile_id: &str,
) -> Option<&'a StoredKey> {
    keys.iter()
        .find(|key| key.profile_ids.iter().any(|id| id == profile_id))
}

fn list_keys_at(path: &Path, existing_profiles: &[String]) -> Result<Vec<StoredKey>, String> {
    let _guard = lock()?;
    let mut keys = read_keys(path)?;
    if prune_assignments(&mut keys, existing_profiles) {
        write_keys(path, &keys)?;
    }
    Ok(keys)
}

pub(crate) fn prune_assignments(keys: &mut [StoredKey], existing_profiles: &[String]) -> bool {
    let mut changed = false;
    for key in keys.iter_mut() {
        let before = key.profile_ids.len();
        key.profile_ids.retain(|id| existing_profiles.contains(id));
        changed |= key.profile_ids.len() != before;
    }
    changed
}

pub(crate) fn clean_name(name: &str) -> Result<String, String> {
    let name: String = name
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME_CHARS)
        .collect();
    if name.is_empty() {
        Err("Give the key a name".to_string())
    } else {
        Ok(name)
    }
}

/// Comments end up in authorized_keys and in scripts quoted for sh and
/// PowerShell, so they keep to characters safe in both.
pub(crate) fn clean_comment(comment: &str) -> String {
    comment
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-@:".contains(c) {
                c
            } else {
                '-'
            }
        })
        .take(MAX_NAME_CHARS)
        .collect()
}

/// Makes a key pair and keeps it on this computer. Contacts no server. The
/// vault entry is rolled back if the list cannot be saved.
fn generate_key_at(
    path: &Path,
    credentials: &impl CredentialStore,
    name: &str,
    kind: KeyKind,
    comment: &str,
) -> Result<Vec<StoredKey>, String> {
    let name = clean_name(name)?;
    let comment = clean_comment(comment);
    let generated = generate_kind(kind, &comment)?;
    let id = Uuid::new_v4().to_string();
    credentials
        .set_password(&key_vault_key(&id), generated.private_openssh.to_string())
        .map_err(|error| format!("The key could not be saved in the vault: {error}"))?;
    let stored = StoredKey {
        id: id.clone(),
        name,
        kind,
        public_key: generated.public_line,
        fingerprint: generated.fingerprint,
        created_at: now_epoch_seconds(),
        profile_ids: Vec::new(),
    };
    update_keys(path, |keys| {
        keys.push(stored);
        Ok(())
    })
    .inspect_err(|_| {
        let _ = credentials.clear_password(&key_vault_key(&id));
    })
}

fn rename_key_at(path: &Path, key_id: &str, name: &str) -> Result<Vec<StoredKey>, String> {
    let name = clean_name(name)?;
    update_keys(path, |keys| {
        find_mut(keys, key_id)?.name = name;
        Ok(())
    })
}

/// Forgets the key on this computer. Servers keep it in authorized_keys; the
/// profiles that used it fall back to their other credentials.
fn delete_key_at(
    path: &Path,
    credentials: &impl CredentialStore,
    key_id: &str,
) -> Result<Vec<StoredKey>, String> {
    let keys = update_keys(path, |keys| {
        let index = keys
            .iter()
            .position(|key| key.id == key_id)
            .ok_or_else(|| "That SSH key no longer exists".to_string())?;
        keys.remove(index);
        Ok(())
    })?;
    if let Err(error) = credentials.clear_password(&key_vault_key(key_id)) {
        eprintln!("GpuTerm: could not erase a deleted SSH key from the vault: {error}");
    }
    Ok(keys)
}

fn assign_key_at(
    path: &Path,
    profile_id: &str,
    key_id: Option<&str>,
) -> Result<Vec<StoredKey>, String> {
    update_keys(path, |keys| assign(keys, profile_id, key_id))
}

/// One key per profile; one key may serve many profiles.
pub(crate) fn assign(
    keys: &mut [StoredKey],
    profile_id: &str,
    key_id: Option<&str>,
) -> Result<(), String> {
    if let Some(key_id) = key_id {
        if !keys.iter().any(|key| key.id == key_id) {
            return Err("That SSH key no longer exists".to_string());
        }
    }
    for key in keys.iter_mut() {
        key.profile_ids.retain(|id| id != profile_id);
        if Some(key.id.as_str()) == key_id {
            key.profile_ids.push(profile_id.to_string());
        }
    }
    Ok(())
}

fn find_key(path: &Path, key_id: &str) -> Result<StoredKey, String> {
    let _guard = lock()?;
    read_keys(path)?
        .into_iter()
        .find(|key| key.id == key_id)
        .ok_or_else(|| "That SSH key no longer exists".to_string())
}

fn find_mut<'a>(keys: &'a mut [StoredKey], key_id: &str) -> Result<&'a mut StoredKey, String> {
    keys.iter_mut()
        .find(|key| key.id == key_id)
        .ok_or_else(|| "That SSH key no longer exists".to_string())
}

fn load_private(
    credentials: &impl CredentialStore,
    key_id: &str,
) -> Result<Zeroizing<String>, String> {
    credentials
        .get_password(&key_vault_key(key_id))?
        .map(Zeroizing::new)
        .ok_or_else(|| {
            "The key is not in the vault on this computer. Generate a new key.".to_string()
        })
}

/// The vault must be open for anything that reads or writes a private key.
fn require_unlocked(credentials: &SecureCredentialStore) -> Result<(), String> {
    if credentials.status().unlocked {
        Ok(())
    } else {
        Err(
            "Unlock the credential vault with the GpuTerm master password first: \
             private keys are kept there."
                .to_string(),
        )
    }
}

fn existing_profile_ids() -> Result<Vec<String>, String> {
    Ok(list_profiles()?
        .into_iter()
        .map(|profile| profile.id)
        .collect())
}

async fn blocking<T: Send + 'static>(
    label: &str,
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| format!("{label} task failed: {error}"))?
}

/// Keys, without assignments to profiles that no longer exist.
#[tauri::command]
pub async fn list_keys() -> Result<Vec<StoredKey>, String> {
    blocking("SSH key list", || {
        let profiles = existing_profile_ids()?;
        list_keys_at(&keys_path(), &profiles)
    })
    .await
}

#[tauri::command]
pub async fn generate_key(
    state: State<'_, AppState>,
    name: String,
    kind: String,
    comment: Option<String>,
) -> Result<Vec<StoredKey>, String> {
    let kind = KeyKind::parse(&kind)?;
    require_unlocked(&state.credentials)?;
    let credentials = state.credentials.clone();
    blocking("Key generation", move || {
        generate_key_at(
            &keys_path(),
            &credentials,
            &name,
            kind,
            comment.as_deref().unwrap_or_default(),
        )
    })
    .await
}

#[tauri::command]
pub async fn rename_key(key_id: String, name: String) -> Result<Vec<StoredKey>, String> {
    blocking("Key rename", move || {
        rename_key_at(&keys_path(), &key_id, &name)
    })
    .await
}

#[tauri::command]
pub async fn delete_key(
    state: State<'_, AppState>,
    key_id: String,
) -> Result<Vec<StoredKey>, String> {
    require_unlocked(&state.credentials)?;
    let credentials = state.credentials.clone();
    blocking("Key deletion", move || {
        delete_key_at(&keys_path(), &credentials, &key_id)
    })
    .await
}

/// Makes `key_id` the key `profile_id` signs in with, or none. Does not
/// touch the server: for a key installed there already, or by hand.
#[tauri::command]
pub async fn assign_key(
    profile_id: String,
    key_id: Option<String>,
) -> Result<Vec<StoredKey>, String> {
    blocking("Key assignment", move || {
        if key_id.is_some() && find_profile(&profile_id)?.is_local {
            return Err("A local terminal does not use SSH keys".to_string());
        }
        assign_key_at(&keys_path(), &profile_id, key_id.as_deref())
    })
    .await
}

/// Adds the public key to the profile's server over a connection signed in
/// the usual way, checks that the key alone signs in, then assigns it.
#[tauri::command]
pub async fn install_key(
    app: AppHandle,
    state: State<'_, AppState>,
    profile_id: String,
    key_id: String,
) -> Result<Vec<StoredKey>, String> {
    let profile = find_profile(&profile_id)?;
    if profile.is_local {
        return Err("A local terminal does not use SSH keys".to_string());
    }
    require_unlocked(&state.credentials)?;
    let stored = find_key(&keys_path(), &key_id)?;
    let secret = SecretKey::from_openssh(load_private(&state.credentials, &key_id)?)?;
    let mut target =
        target_for_profile(&state.active_connections, &state.credentials, &profile_id)?;
    attach_managed_keys(&mut target, &state.credentials);
    let mut auth = sign_in_responder(&app, &state, &profile_id, true);
    let os_cache = Arc::clone(&state.remote_os_cache);

    blocking("Key install", move || {
        let connection = open_ssh_session_with(&target, auth.as_mut())?;
        let session = connection.session();
        let os = resolve_remote_os(&os_cache, &profile_id, session);
        install(session, os, &stored.public_key)?;
        verify_managed_key_session(&key_only(&target, secret)).map_err(|error| {
            format!(
                "The key was added to authorized_keys, but the server did not accept it. Check \
                 that the home directory and ~/.ssh are not writable by others (sshd \
                 StrictModes) and that the server allows this key type. ({error})"
            )
        })?;
        assign_key_at(&keys_path(), &profile_id, Some(&key_id))
    })
    .await
}

/// Writes the private key to `path` (encrypted with `passphrase`, or as is
/// when the user chose no passphrase) and its public key to `path.pub`.
#[tauri::command]
pub async fn export_private_key(
    state: State<'_, AppState>,
    key_id: String,
    path: String,
    passphrase: Option<String>,
) -> Result<(), String> {
    let passphrase = passphrase.map(Zeroizing::new);
    require_unlocked(&state.credentials)?;
    let credentials = state.credentials.clone();
    blocking("Key export", move || {
        let stored = find_key(&keys_path(), &key_id)?;
        let private = load_private(&credentials, &key_id)?;
        let contents = match passphrase.as_deref() {
            Some(passphrase) => render_export(&private, passphrase)?,
            None => private,
        };
        let path = PathBuf::from(path);
        write_private_file(&path, contents.as_bytes())?;
        let mut public = path.into_os_string();
        public.push(".pub");
        write_private_file(
            Path::new(&public),
            format!("{}\n", stored.public_key).as_bytes(),
        )
    })
    .await
}

/// Writes only the public key, as `ssh-keygen` would to `id_*.pub`.
#[tauri::command]
pub async fn save_public_key(key_id: String, path: String) -> Result<(), String> {
    blocking("Public key save", move || {
        let stored = find_key(&keys_path(), &key_id)?;
        write_private_file(
            Path::new(&path),
            format!("{}\n", stored.public_key).as_bytes(),
        )
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh::credentials::MemoryCredentialStore;
    use ssh_key::{HashAlg, PrivateKey, PublicKey};

    fn stored(id: &str, profiles: &[&str]) -> StoredKey {
        StoredKey {
            id: id.to_string(),
            name: id.to_string(),
            kind: KeyKind::Ed25519,
            public_key: String::new(),
            fingerprint: String::new(),
            created_at: 0,
            profile_ids: profiles.iter().map(|id| id.to_string()).collect(),
        }
    }

    fn temp_list() -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("gputerm-keys-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        (dir.join("keys.json"), dir)
    }

    #[test]
    fn every_key_type_round_trips_through_openssh() {
        for (kind, algorithm) in [
            (KeyKind::Ed25519, "ssh-ed25519"),
            (KeyKind::Rsa3072, "ssh-rsa"),
            (KeyKind::EcdsaP256, "ecdsa-sha2-nistp256"),
            (KeyKind::EcdsaP384, "ecdsa-sha2-nistp384"),
            (KeyKind::EcdsaP521, "ecdsa-sha2-nistp521"),
        ] {
            let key = generate_kind(kind, "lab@gputerm").unwrap();
            assert!(
                key.public_line.starts_with(&format!("{algorithm} ")),
                "{kind:?}"
            );
            assert!(key.public_line.ends_with(" lab@gputerm"));
            let private = PrivateKey::from_openssh(key.private_openssh.as_str()).unwrap();
            let public = PublicKey::from_openssh(&key.public_line).unwrap();
            assert_eq!(private.public_key().key_data(), public.key_data());
            assert_eq!(
                public.fingerprint(HashAlg::Sha256).to_string(),
                key.fingerprint
            );
            crate::ssh::managed_key::validate_public_line(&key.public_line).unwrap();
            // A key manager key also signs in from memory like a managed key.
            SecretKey::from_openssh(key.private_openssh.clone()).unwrap();
        }
    }

    #[test]
    fn rsa_keys_have_the_chosen_size() {
        let key = generate_kind(KeyKind::Rsa4096, "big").unwrap();
        let private = PrivateKey::from_openssh(key.private_openssh.as_str()).unwrap();
        let rsa = private.key_data().rsa().unwrap();
        assert_eq!(rsa.public.n.as_positive_bytes().unwrap().len() * 8, 4096);
    }

    #[test]
    fn key_types_parse_from_their_names() {
        assert_eq!(KeyKind::parse("ed25519").unwrap(), KeyKind::Ed25519);
        assert_eq!(KeyKind::parse("rsa-4096").unwrap(), KeyKind::Rsa4096);
        assert_eq!(KeyKind::parse("ecdsa-p521").unwrap(), KeyKind::EcdsaP521);
        assert!(KeyKind::parse("dsa").is_err());
        assert!(KeyKind::parse("rsa-1024").is_err());
    }

    #[test]
    fn a_profile_signs_in_with_one_key_at_a_time() {
        let mut keys = vec![stored("a", &["p1"]), stored("b", &["p2"])];
        assign(&mut keys, "p1", Some("b")).unwrap();
        assert!(keys[0].profile_ids.is_empty());
        assert_eq!(keys[1].profile_ids, vec!["p2", "p1"]);
        assert_eq!(key_for_profile(&keys, "p1").unwrap().id, "b");
        // Several profiles may share one key.
        assert_eq!(key_for_profile(&keys, "p2").unwrap().id, "b");

        assign(&mut keys, "p1", None).unwrap();
        assert!(key_for_profile(&keys, "p1").is_none());
        assert!(assign(&mut keys, "p1", Some("gone")).is_err());
    }

    #[test]
    fn deleted_profiles_lose_their_assignment() {
        let mut keys = vec![stored("a", &["p1", "gone"])];
        assert!(prune_assignments(&mut keys, &["p1".to_string()]));
        assert_eq!(keys[0].profile_ids, vec!["p1"]);
        assert!(!prune_assignments(&mut keys, &["p1".to_string()]));
    }

    #[test]
    fn names_and_comments_are_kept_safe() {
        assert!(clean_name("  \n ").is_err());
        assert_eq!(clean_name(" Lab laptop ").unwrap(), "Lab laptop");
        assert_eq!(clean_name("a\u{7}b").unwrap(), "ab");
        assert_eq!(clean_name(&"x".repeat(100)).unwrap().len(), MAX_NAME_CHARS);
        assert_eq!(clean_comment("me@host; rm -rf ~"), "me@host--rm--rf--");
        assert_eq!(clean_comment(""), "");
    }

    #[test]
    fn the_key_list_reads_older_files_without_assignments() {
        let json = r#"[{"id":"k","name":"n","kind":"ecdsa-p256","publicKey":"p","fingerprint":"f","createdAt":1}]"#;
        let keys: Vec<StoredKey> = serde_json::from_str(json).unwrap();
        assert_eq!(keys[0].kind, KeyKind::EcdsaP256);
        assert!(keys[0].profile_ids.is_empty());
        let written = serde_json::to_value(&keys[0]).unwrap();
        assert_eq!(written["publicKey"], "p");
        assert_eq!(written["profileIds"], serde_json::json!([]));
    }

    #[test]
    fn keys_live_in_the_vault_and_the_list_holds_only_public_parts() {
        let (path, dir) = temp_list();
        let vault = MemoryCredentialStore::default();
        let keys =
            generate_key_at(&path, &vault, " Lab laptop ", KeyKind::EcdsaP256, "me@lab").unwrap();
        assert_eq!(keys.len(), 1);
        let key = &keys[0];
        assert_eq!(key.name, "Lab laptop");
        assert!(key.public_key.starts_with("ecdsa-sha2-nistp256 "));
        let private = vault
            .get_password(&key_vault_key(&key.id))
            .unwrap()
            .unwrap();
        assert!(private.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----"));
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(!on_disk.contains("PRIVATE KEY"));

        let renamed = rename_key_at(&path, &key.id, "Desk").unwrap();
        assert_eq!(renamed[0].name, "Desk");
        let assigned = assign_key_at(&path, "p1", Some(&key.id)).unwrap();
        assert_eq!(assigned[0].profile_ids, vec!["p1"]);
        assert_eq!(list_keys_at(&path, &[]).unwrap()[0].profile_ids.len(), 0);

        assert!(delete_key_at(&path, &vault, &key.id).unwrap().is_empty());
        assert_eq!(vault.get_password(&key_vault_key(&key.id)).unwrap(), None);
        assert!(delete_key_at(&path, &vault, &key.id).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_key_whose_list_cannot_be_saved_leaves_nothing_in_the_vault() {
        let (path, dir) = temp_list();
        // A directory where the list file should be makes the write fail.
        std::fs::create_dir_all(&path).unwrap();
        let vault = MemoryCredentialStore::default();
        assert!(generate_key_at(&path, &vault, "n", KeyKind::Ed25519, "").is_err());
        assert!(vault.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
