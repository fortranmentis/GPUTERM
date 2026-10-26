//! SSH keys GpuTerm creates and manages for a profile.
//!
//! The private key lives only in the encrypted credential vault and is handed
//! to libssh2 from memory, never written to disk unless the user exports it
//! (always passphrase-encrypted). The public key is installed into the
//! server's `authorized_keys` over a connection signed in the usual way, and
//! checked by signing in with the new key alone before it is relied on.

use serde::{Deserialize, Serialize};
use ssh_key::{
    private::{KeypairData, RsaKeypair},
    rand_core::OsRng,
    Algorithm, EcdsaCurve, HashAlg, LineEnding, PrivateKey,
};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, State};
use zeroize::Zeroizing;

use super::credentials::CredentialStore;
use super::resource_details::resolve_remote_os;
use super::session::{
    list_profiles, open_ssh_session_with, set_managed_key_info, target_for_profile,
    verify_managed_key_session, AppState, SessionProfile, SshTarget,
};
use super::system_monitor::{
    now_epoch_seconds, run_remote_command_for, run_remote_command_with_budget, shell_quote,
    RemoteOs,
};
use super::terminal::sign_in_responder;

/// Shortest passphrase accepted for an exported private key.
pub const MIN_EXPORT_PASSPHRASE_CHARS: usize = 8;
const INSTALL_BUDGET_SECS: u64 = 15;

/// What a profile records about its managed key. Nothing here is secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedKeyInfo {
    /// `ssh-ed25519 <base64> <comment>`, as installed on the server.
    pub public_key: String,
    /// `SHA256:...`
    pub fingerprint: String,
    pub created_at: u64,
    /// When the server last accepted a sign-in with it.
    pub installed_at: u64,
}

/// A private key held in memory for signing in. Never printed.
#[derive(Clone)]
pub struct SecretKey {
    pub private_openssh: Zeroizing<String>,
    pub fingerprint: String,
}

impl std::fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SecretKey({}, <redacted>)", self.fingerprint)
    }
}

impl SecretKey {
    pub fn from_openssh(private_openssh: Zeroizing<String>) -> Result<Self, String> {
        let key = PrivateKey::from_openssh(private_openssh.as_str())
            .map_err(|error| format!("The stored SSH key is unreadable: {error}"))?;
        Ok(Self {
            fingerprint: key.public_key().fingerprint(HashAlg::Sha256).to_string(),
            private_openssh,
        })
    }
}

/// The vault entry holding a profile's private key. Kept apart from the
/// profile's own entry (its password), which a connect without a password
/// clears.
pub fn managed_key_vault_key(profile_id: &str) -> String {
    format!("sshkey:{profile_id}")
}

/// Loads the managed key of every hop in `target` that has one, and the key
/// manager key assigned to it. A locked vault or a missing entry leaves the
/// hop to its other credentials.
pub fn attach_managed_keys(target: &mut SshTarget, credentials: &impl CredentialStore) {
    attach_managed_keys_with(
        target,
        &|account: &str| credentials.get_password(account).ok().flatten(),
        &super::keyring::assigned_key_id,
    );
}

/// `load` reads a vault entry; `assigned` names the key manager key a
/// profile signs in with. Sign-in tries the profile's own managed key first,
/// then the assigned key, then the profile's other credentials.
pub(crate) fn attach_managed_keys_with(
    target: &mut SshTarget,
    load: &dyn Fn(&str) -> Option<String>,
    assigned: &dyn Fn(&str) -> Option<String>,
) {
    let read = |account: &str| {
        load(account).and_then(|private| {
            SecretKey::from_openssh(Zeroizing::new(private))
                .inspect_err(|error| eprintln!("GpuTerm: {error}"))
                .ok()
        })
    };
    let mut hop = Some(target);
    while let Some(current) = hop {
        if current.managed_key.is_none() {
            current.managed_key = read(&managed_key_vault_key(&current.session_id));
        }
        if current.assigned_key.is_none() {
            current.assigned_key = assigned(&current.session_id)
                .and_then(|key_id| read(&super::keyring::key_vault_key(&key_id)))
                // The same key twice would only spend another failed attempt.
                .filter(|key| {
                    current
                        .managed_key
                        .as_ref()
                        .is_none_or(|own| own.fingerprint != key.fingerprint)
                });
        }
        hop = current.proxy.as_deref_mut();
    }
}

/// Key types GpuTerm can generate: what `ssh-keygen -t` offers, minus DSA.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyKind {
    #[serde(rename = "ed25519")]
    Ed25519,
    #[serde(rename = "rsa-3072")]
    Rsa3072,
    #[serde(rename = "rsa-4096")]
    Rsa4096,
    #[serde(rename = "ecdsa-p256")]
    EcdsaP256,
    #[serde(rename = "ecdsa-p384")]
    EcdsaP384,
    #[serde(rename = "ecdsa-p521")]
    EcdsaP521,
}

impl KeyKind {
    pub fn parse(value: &str) -> Result<Self, String> {
        serde_json::from_value(serde_json::Value::String(value.to_string()))
            .map_err(|_| format!("Unknown SSH key type: {value}"))
    }

    fn private_key(self) -> Result<PrivateKey, ssh_key::Error> {
        let ecdsa = |curve| PrivateKey::random(&mut OsRng, Algorithm::Ecdsa { curve });
        match self {
            KeyKind::Ed25519 => PrivateKey::random(&mut OsRng, Algorithm::Ed25519),
            KeyKind::Rsa3072 => rsa(3072),
            KeyKind::Rsa4096 => rsa(4096),
            KeyKind::EcdsaP256 => ecdsa(EcdsaCurve::NistP256),
            KeyKind::EcdsaP384 => ecdsa(EcdsaCurve::NistP384),
            KeyKind::EcdsaP521 => ecdsa(EcdsaCurve::NistP521),
        }
    }
}

fn rsa(bits: usize) -> Result<PrivateKey, ssh_key::Error> {
    PrivateKey::new(KeypairData::from(RsaKeypair::random(&mut OsRng, bits)?), "")
}

/// A freshly generated key pair.
pub struct GeneratedKey {
    /// OpenSSH private key format, unencrypted (the vault encrypts it).
    pub private_openssh: Zeroizing<String>,
    /// `<algorithm> <base64> <comment>`, one line.
    pub public_line: String,
    /// `SHA256:...`, as `ssh-keygen -l` prints it.
    pub fingerprint: String,
}

/// An Ed25519 key, as profile setup creates.
pub fn generate(comment: &str) -> Result<GeneratedKey, String> {
    generate_kind(KeyKind::Ed25519, comment)
}

pub fn generate_kind(kind: KeyKind, comment: &str) -> Result<GeneratedKey, String> {
    let mut key = kind
        .private_key()
        .map_err(|error| format!("Could not generate an SSH key: {error}"))?;
    key.set_comment(comment);
    let private_openssh = key
        .to_openssh(LineEnding::LF)
        .map_err(|error| format!("Could not encode the SSH key: {error}"))?;
    let public = key.public_key();
    let public_line = public
        .to_openssh()
        .map_err(|error| format!("Could not encode the public key: {error}"))?;
    Ok(GeneratedKey {
        private_openssh,
        public_line,
        fingerprint: public.fingerprint(HashAlg::Sha256).to_string(),
    })
}

/// `gputerm:<profile>:<date>`, limited to characters every shell and
/// `authorized_keys` parser accepts as-is.
pub fn key_comment(profile_name: &str, now_epoch: u64) -> String {
    let name: String = profile_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-@".contains(c) {
                c
            } else {
                '-'
            }
        })
        .take(48)
        .collect();
    let date = chrono::DateTime::from_timestamp(now_epoch as i64, 0)
        .map(|time| time.format("%Y-%m-%d").to_string())
        .unwrap_or_default();
    format!("gputerm:{name}:{date}")
}

/// Public-key algorithms GpuTerm generates and installs.
const INSTALLABLE_ALGORITHMS: &[&str] = &[
    "ssh-ed25519",
    "ssh-rsa",
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
];

/// Accepts only `<algorithm> <base64> [comment]` for a known algorithm with
/// a plain comment, so the line is safe inside single quotes in both sh and
/// PowerShell.
pub(crate) fn validate_public_line(line: &str) -> Result<(), String> {
    let mut parts = line.split(' ');
    let algorithm = parts.next().unwrap_or_default();
    let body = parts.next().unwrap_or_default();
    let comment: Vec<&str> = parts.collect();
    let valid = INSTALLABLE_ALGORITHMS.contains(&algorithm)
        && !body.is_empty()
        && body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+/=".contains(c))
        && comment.len() <= 1
        && comment.iter().all(|comment| {
            comment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-@:".contains(c))
        });
    if valid {
        Ok(())
    } else {
        Err("Refusing to install a malformed public key".to_string())
    }
}

/// Appends the key to `~/.ssh/authorized_keys` unless it is already there,
/// creating the directory and file with the permissions sshd's StrictModes
/// requires.
pub(crate) fn posix_install_script(public_line: &str) -> String {
    let key = shell_quote(public_line);
    format!(
        "umask 077\n\
         mkdir -p \"$HOME/.ssh\" && chmod 700 \"$HOME/.ssh\" || exit 1\n\
         f=\"$HOME/.ssh/authorized_keys\"\n\
         touch \"$f\" && chmod 600 \"$f\" || exit 1\n\
         key={key}\n\
         if ! grep -qxF \"$key\" \"$f\"; then\n\
         \x20 if [ -s \"$f\" ] && [ -n \"$(tail -c 1 \"$f\")\" ]; then printf '\\n' >> \"$f\" || exit 1; fi\n\
         \x20 printf '%s\\n' \"$key\" >> \"$f\" || exit 1\n\
         fi\n\
         echo __OK__"
    )
}

/// Removes exactly that line, writing a temporary copy and renaming it so a
/// failure never leaves a truncated file.
pub(crate) fn posix_remove_script(public_line: &str) -> String {
    let key = shell_quote(public_line);
    format!(
        "f=\"$HOME/.ssh/authorized_keys\"\n\
         [ -f \"$f\" ] || {{ echo __OK__; exit 0; }}\n\
         umask 077\n\
         key={key}\n\
         tmp=\"$f.gputerm.$$\"\n\
         grep -vxF \"$key\" \"$f\" > \"$tmp\"\n\
         [ $? -le 1 ] || {{ rm -f \"$tmp\"; exit 1; }}\n\
         chmod 600 \"$tmp\" && mv \"$tmp\" \"$f\" || {{ rm -f \"$tmp\"; exit 1; }}\n\
         echo __OK__"
    )
}

/// Windows OpenSSH reads administrators' keys from one machine-wide file
/// that only Administrators and SYSTEM may access; everyone else uses their
/// profile's `.ssh\authorized_keys`. Groups are matched by SID, which is the
/// same on every language edition.
const WINDOWS_KEY_FILE: &str = "$admins = New-Object Security.Principal.SecurityIdentifier 'S-1-5-32-544'\n\
$isAdmin = [Security.Principal.WindowsIdentity]::GetCurrent().Groups -contains $admins\n\
if ($isAdmin) { $file = Join-Path $env:ProgramData 'ssh\\administrators_authorized_keys' }\n\
else { $dir = Join-Path $env:USERPROFILE '.ssh'; New-Item -ItemType Directory -Force -Path $dir | Out-Null; $file = Join-Path $dir 'authorized_keys' }";

pub(crate) fn windows_install_script(public_line: &str) -> String {
    format!(
        "$ErrorActionPreference = 'Stop'\n$key = '{public_line}'\n{WINDOWS_KEY_FILE}\n\
         if (-not (Test-Path -LiteralPath $file)) {{ New-Item -ItemType File -Force -Path $file | Out-Null }}\n\
         $lines = @(Get-Content -LiteralPath $file -ErrorAction SilentlyContinue)\n\
         if ($lines -notcontains $key) {{ Add-Content -LiteralPath $file -Value $key -Encoding ascii }}\n\
         if ($isAdmin) {{\n\
         \x20 icacls $file /inheritance:r /grant '*S-1-5-32-544:F' /grant '*S-1-5-18:F' | Out-Null\n\
         \x20 if ($LASTEXITCODE -ne 0) {{ throw 'Could not secure authorized_keys permissions' }}\n\
         }}\n\
         '__OK__'"
    )
}

pub(crate) fn windows_remove_script(public_line: &str) -> String {
    format!(
        "$ErrorActionPreference = 'Stop'\n$key = '{public_line}'\n{WINDOWS_KEY_FILE}\n\
         if (Test-Path -LiteralPath $file) {{\n\
         \x20 $kept = @(Get-Content -LiteralPath $file | Where-Object {{ $_ -ne $key }})\n\
         \x20 Set-Content -LiteralPath $file -Value $kept -Encoding ascii\n\
         }}\n\
         '__OK__'"
    )
}

fn run_key_script(
    session: &ssh2::Session,
    os: RemoteOs,
    posix: String,
    windows: String,
) -> Result<(), String> {
    let output = if os == RemoteOs::Windows {
        run_remote_command_for(session, os, &windows)
    } else {
        run_remote_command_with_budget(session, &posix, INSTALL_BUDGET_SECS)
    }?;
    if output.lines().any(|line| line.trim() == "__OK__") {
        Ok(())
    } else {
        Err(format!(
            "The server did not confirm the change: {}",
            output.trim()
        ))
    }
}

pub(crate) fn install(session: &ssh2::Session, os: RemoteOs, public_line: &str) -> Result<(), String> {
    validate_public_line(public_line)?;
    run_key_script(
        session,
        os,
        posix_install_script(public_line),
        windows_install_script(public_line),
    )
    .map_err(|error| format!("Could not add the key to authorized_keys: {error}"))
}

fn uninstall(session: &ssh2::Session, os: RemoteOs, public_line: &str) -> Result<(), String> {
    validate_public_line(public_line)?;
    run_key_script(
        session,
        os,
        posix_remove_script(public_line),
        windows_remove_script(public_line),
    )
    .map_err(|error| format!("Could not remove the key from authorized_keys: {error}"))
}

/// The private key, encrypted with `passphrase` in OpenSSH format
/// (bcrypt-pbkdf, aes256-ctr), as `ssh-keygen` writes it.
pub fn render_export(private_openssh: &str, passphrase: &str) -> Result<Zeroizing<String>, String> {
    if passphrase.chars().count() < MIN_EXPORT_PASSPHRASE_CHARS {
        return Err(format!(
            "Use a passphrase of at least {MIN_EXPORT_PASSPHRASE_CHARS} characters"
        ));
    }
    PrivateKey::from_openssh(private_openssh)
        .and_then(|key| key.encrypt(&mut OsRng, passphrase))
        .and_then(|key| key.to_openssh(LineEnding::LF))
        .map_err(|error| format!("Could not encrypt the key for export: {error}"))
}

/// Writes through a temporary file readable only by the user, then renames
/// it into place.
pub(crate) fn write_private_file(path: &Path, contents: &[u8]) -> Result<(), String> {
    let temp = path.with_file_name(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("key"),
        uuid::Uuid::new_v4()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = options
        .open(&temp)
        .and_then(|mut file| {
            file.write_all(contents)?;
            file.sync_all()
        })
        .and_then(|_| fs::rename(&temp, path));
    if let Err(error) = result {
        let _ = fs::remove_file(&temp);
        return Err(format!("Could not write {}: {error}", path.display()));
    }
    Ok(())
}

pub(crate) fn find_profile(profile_id: &str) -> Result<SessionProfile, String> {
    list_profiles()?
        .into_iter()
        .find(|profile| profile.id == profile_id)
        .ok_or_else(|| "That SSH session profile no longer exists".to_string())
}

/// The target with its own sign-in credentials replaced by `key` alone, so a
/// sign-in proves the server accepts that key. Jump hosts keep theirs.
pub(crate) fn key_only(target: &SshTarget, key: SecretKey) -> SshTarget {
    SshTarget {
        password: None,
        private_key_path: None,
        managed_key: Some(key),
        assigned_key: None,
        ..target.clone()
    }
}

/// Creates a key for the profile, installs it on the server, confirms the
/// server accepts it, and only then keeps it. Replaces an existing key the
/// same way, removing the old one from the server afterwards.
#[tauri::command]
pub async fn setup_managed_key(
    app: AppHandle,
    state: State<'_, AppState>,
    profile_id: String,
) -> Result<Vec<SessionProfile>, String> {
    let profile = find_profile(&profile_id)?;
    if profile.is_local {
        return Err("A local terminal does not use SSH keys".to_string());
    }
    let mut target =
        target_for_profile(&state.active_connections, &state.credentials, &profile_id)?;
    attach_managed_keys(&mut target, &state.credentials);
    let mut auth = sign_in_responder(&app, &state, &profile_id, true);
    let credentials = state.credentials.clone();
    let os_cache = Arc::clone(&state.remote_os_cache);

    tauri::async_runtime::spawn_blocking(move || {
        let connection = open_ssh_session_with(&target, auth.as_mut())?;
        let session = connection.session();
        let os = resolve_remote_os(&os_cache, &profile_id, session);

        let now = now_epoch_seconds();
        let key = generate(&key_comment(&profile.name, now))?;
        install(session, os, &key.public_line)?;

        let secret = SecretKey::from_openssh(key.private_openssh.clone())?;
        let verified = verify_managed_key_session(&key_only(&target, secret));
        if let Err(error) = verified {
            let cleanup = match uninstall(session, os, &key.public_line) {
                Ok(()) => "The new public key was removed again.".to_string(),
                Err(cleanup_error) => format!(
                    "The new public key could not be removed; revoke {} on the server. ({cleanup_error})",
                    key.fingerprint
                ),
            };
            return Err(format!(
                "The server did not accept the new key. {cleanup} Check that the \
                 home directory and ~/.ssh are not writable by others (sshd StrictModes) and that \
                 the server allows key sign-in without a second factor. ({error})"
            ));
        }

        if let Err(error) = credentials.set_password(
            &managed_key_vault_key(&profile_id),
            key.private_openssh.to_string(),
        ) {
            let cleanup = uninstall(session, os, &key.public_line)
                .err()
                .map(|cleanup_error| format!(
                    " Public-key cleanup also failed; revoke {} on the server. ({cleanup_error})",
                    key.fingerprint
                ))
                .unwrap_or_default();
            return Err(format!("The key could not be saved in the vault: {error}.{cleanup}"));
        }
        // Replacing: the old key leaves the server only once the new one works.
        if let Some(previous) = profile.managed_key.as_ref() {
            if previous.public_key != key.public_line {
                if let Err(error) = uninstall(session, os, &previous.public_key) {
                    eprintln!("GpuTerm: {error}");
                }
            }
        }
        set_managed_key_info(
            &profile_id,
            Some(ManagedKeyInfo {
                public_key: key.public_line,
                fingerprint: key.fingerprint,
                created_at: now,
                installed_at: now,
            }),
        )
    })
    .await
    .map_err(|error| format!("Key setup task failed: {error}"))?
}

/// Forgets the profile's managed key; with `remove_from_server`, first takes
/// it out of the server's authorized_keys (failing if that cannot be done, so
/// the user can retry or choose to keep it there).
#[tauri::command]
pub async fn remove_managed_key(
    app: AppHandle,
    state: State<'_, AppState>,
    profile_id: String,
    remove_from_server: bool,
) -> Result<Vec<SessionProfile>, String> {
    let profile = find_profile(&profile_id)?;
    let Some(info) = profile.managed_key.clone() else {
        return list_profiles();
    };
    let credentials = state.credentials.clone();
    let server = if remove_from_server {
        let mut target =
            target_for_profile(&state.active_connections, &state.credentials, &profile_id)?;
        attach_managed_keys(&mut target, &state.credentials);
        Some((
            target,
            sign_in_responder(&app, &state, &profile_id, true),
            Arc::clone(&state.remote_os_cache),
        ))
    } else {
        None
    };

    tauri::async_runtime::spawn_blocking(move || {
        if let Some((target, mut auth, os_cache)) = server {
            let connection = open_ssh_session_with(&target, auth.as_mut())?;
            let os = resolve_remote_os(&os_cache, &profile_id, connection.session());
            uninstall(connection.session(), os, &info.public_key)?;
        }
        credentials.clear_password(&managed_key_vault_key(&profile_id))?;
        set_managed_key_info(&profile_id, None)
    })
    .await
    .map_err(|error| format!("Key removal task failed: {error}"))?
}

/// Writes the private key, encrypted with `passphrase`, to `path`, and its
/// public key to `path.pub`.
#[tauri::command]
pub async fn export_managed_key(
    state: State<'_, AppState>,
    profile_id: String,
    path: String,
    passphrase: String,
) -> Result<(), String> {
    let passphrase = Zeroizing::new(passphrase);
    let profile = find_profile(&profile_id)?;
    let info = profile
        .managed_key
        .ok_or_else(|| "This profile has no GpuTerm-managed key".to_string())?;
    let credentials = state.credentials.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let private = credentials
            .get_password(&managed_key_vault_key(&profile_id))?
            .map(Zeroizing::new)
            .ok_or_else(|| {
                "The key is not in the vault. Unlock the vault, or set the key up again."
                    .to_string()
            })?;
        let encrypted = render_export(&private, &passphrase)?;
        let path = PathBuf::from(path);
        write_private_file(&path, encrypted.as_bytes())?;
        let mut public = path.clone().into_os_string();
        public.push(".pub");
        write_private_file(
            Path::new(&public),
            format!("{}\n", info.public_key).as_bytes(),
        )
    })
    .await
    .map_err(|error| format!("Key export task failed: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh::credentials::MemoryCredentialStore;

    #[test]
    fn generates_distinct_ed25519_keys() {
        let first = generate("gputerm:test").unwrap();
        let second = generate("gputerm:test").unwrap();
        assert!(first.public_line.starts_with("ssh-ed25519 "));
        assert!(first.public_line.ends_with(" gputerm:test"));
        assert!(first.fingerprint.starts_with("SHA256:"));
        assert!(first
            .private_openssh
            .starts_with("-----BEGIN OPENSSH PRIVATE KEY-----"));
        assert_ne!(first.public_line, second.public_line);
        validate_public_line(&first.public_line).unwrap();
    }

    #[test]
    fn comments_are_safe_and_dated() {
        assert_eq!(
            key_comment("lab a100 (가)", 1_792_000_000),
            "gputerm:lab-a100----:2026-10-14"
        );
        validate_public_line(&format!("ssh-ed25519 AAAA {}", key_comment("x'y;z", 0))).unwrap();
    }

    #[test]
    fn refuses_public_keys_that_could_escape_quotes() {
        for bad in [
            "ssh-dss AAAA x",
            "ecdsa-sha2-nistp224 AAAA x",
            "ssh-ed25519 AAAA' ; rm -rf ~ '",
            "ssh-rsa AAAA' ; rm -rf ~ '",
            "ecdsa-sha2-nistp256 AAAA $(id)",
            "ssh-ed25519 AAAA $(id)",
            "ssh-ed25519 AAAA a b",
            "ssh-ed25519",
        ] {
            assert!(validate_public_line(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn scripts_quote_the_key_and_report_success() {
        let line = "ssh-ed25519 AAAAC3Nza gputerm:lab:2026-10-17";
        let install = posix_install_script(line);
        assert!(install.contains("key='ssh-ed25519 AAAAC3Nza gputerm:lab:2026-10-17'"));
        assert!(install.contains("grep -qxF \"$key\""));
        assert!(install.contains("chmod 700 \"$HOME/.ssh\""));
        assert!(posix_remove_script(line).contains("grep -vxF \"$key\""));
        let windows = windows_install_script(line);
        assert!(windows.contains("$key = 'ssh-ed25519 AAAAC3Nza gputerm:lab:2026-10-17'"));
        assert!(windows.contains("administrators_authorized_keys"));
        assert!(windows.starts_with("$ErrorActionPreference = 'Stop'"));
        assert!(windows.contains("if ($LASTEXITCODE -ne 0) { throw"));
        assert!(windows_remove_script(line).contains("$_ -ne $key"));
        assert!(windows_remove_script(line).starts_with("$ErrorActionPreference = 'Stop'"));
    }

    #[test]
    fn an_export_opens_with_its_passphrase_only() {
        let key = generate("gputerm:test").unwrap();
        assert!(render_export(&key.private_openssh, "short").is_err());
        let exported = render_export(&key.private_openssh, "correct horse").unwrap();
        let encrypted = PrivateKey::from_openssh(exported.as_str()).unwrap();
        assert!(encrypted.is_encrypted());
        assert!(encrypted.decrypt("wrong passphrase").is_err());
        let decrypted = encrypted.decrypt("correct horse").unwrap();
        assert_eq!(
            decrypted.public_key().to_openssh().unwrap(),
            key.public_line
        );
    }

    #[test]
    fn secret_keys_never_print_their_contents() {
        let key = generate("gputerm:test").unwrap();
        let secret = SecretKey::from_openssh(key.private_openssh.clone()).unwrap();
        let printed = format!("{secret:?}");
        assert!(printed.contains(&key.fingerprint));
        assert!(!printed.contains("PRIVATE KEY"));
    }

    fn hop(id: &str, proxy: Option<SshTarget>) -> SshTarget {
        SshTarget {
            session_id: id.into(),
            host: id.into(),
            port: 22,
            username: "u".into(),
            password: None,
            private_key_path: None,
            managed_key: None,
            assigned_key: None,
            proxy: proxy.map(Box::new),
        }
    }

    fn vault_loader(credentials: &MemoryCredentialStore) -> impl Fn(&str) -> Option<String> + '_ {
        |account: &str| credentials.get_password(account).ok().flatten()
    }

    #[test]
    fn every_hop_gets_its_own_managed_key() {
        let credentials = MemoryCredentialStore::default();
        let key = generate("gputerm:jump").unwrap();
        credentials
            .set_password(
                &managed_key_vault_key("jump"),
                key.private_openssh.to_string(),
            )
            .unwrap();
        // A password under the bare id is not mistaken for a key.
        credentials.set_password("gpu", "pw".into()).unwrap();
        let mut target = hop("gpu", Some(hop("jump", None)));
        attach_managed_keys_with(&mut target, &vault_loader(&credentials), &|_| None);
        assert!(target.managed_key.is_none());
        assert_eq!(
            target
                .proxy
                .as_ref()
                .unwrap()
                .managed_key
                .as_ref()
                .unwrap()
                .fingerprint,
            key.fingerprint
        );
    }

    #[test]
    fn an_assigned_key_signs_in_after_the_profiles_own_key() {
        let credentials = MemoryCredentialStore::default();
        let own = generate("gputerm:own").unwrap();
        let shared = generate_kind(KeyKind::EcdsaP256, "shared").unwrap();
        credentials
            .set_password(&managed_key_vault_key("gpu"), own.private_openssh.to_string())
            .unwrap();
        credentials
            .set_password(
                &crate::ssh::keyring::key_vault_key("k1"),
                shared.private_openssh.to_string(),
            )
            .unwrap();
        let assigned = |profile: &str| (profile != "jump").then(|| "k1".to_string());
        let mut target = hop("gpu", Some(hop("jump", None)));
        attach_managed_keys_with(&mut target, &vault_loader(&credentials), &assigned);
        assert_eq!(target.managed_key.as_ref().unwrap().fingerprint, own.fingerprint);
        assert_eq!(target.assigned_key.as_ref().unwrap().fingerprint, shared.fingerprint);
        let jump = target.proxy.as_ref().unwrap();
        assert!(jump.managed_key.is_none() && jump.assigned_key.is_none());

        // Without its own key, a profile signs in with the assigned key alone.
        let mut bare = hop("other", None);
        attach_managed_keys_with(&mut bare, &vault_loader(&credentials), &assigned);
        assert!(bare.managed_key.is_none());
        assert_eq!(bare.assigned_key.as_ref().unwrap().fingerprint, shared.fingerprint);

        // A locked vault (nothing readable) leaves every slot empty.
        let mut locked = hop("gpu", None);
        attach_managed_keys_with(&mut locked, &|_| None, &assigned);
        assert!(locked.managed_key.is_none() && locked.assigned_key.is_none());

        // Verification with one key never carries the assigned key along.
        let verify = key_only(&target, SecretKey::from_openssh(shared.private_openssh).unwrap());
        assert!(verify.assigned_key.is_none());
        assert_eq!(verify.managed_key.unwrap().fingerprint, shared.fingerprint);
    }

    #[test]
    fn an_unreadable_stored_key_is_skipped() {
        let mut target = hop("gpu", None);
        attach_managed_keys_with(&mut target, &|_| Some("not a key".to_string()), &|_| {
            Some("k".to_string())
        });
        assert!(target.managed_key.is_none() && target.assigned_key.is_none());
    }

    #[test]
    fn private_files_are_written_whole_and_private() {
        let dir = std::env::temp_dir().join(format!("gputerm-export-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("id_gputerm");
        write_private_file(&path, b"secret").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"secret");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        // Overwriting an earlier export replaces it.
        write_private_file(&path, b"newer").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"newer");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    /// Live check: signs in to an SSH server with a key held only in memory.
    /// Needs `GPUTERM_TEST_SSH_USER` and `GPUTERM_TEST_SSH_KEYDIR` holding
    /// `id` (see `write_spike_key`) whose public half the server trusts.
    #[test]
    #[ignore]
    fn memory_key_signs_in_to_a_live_server() {
        let user = std::env::var("GPUTERM_TEST_SSH_USER").unwrap();
        let dir = std::env::var("GPUTERM_TEST_SSH_KEYDIR").unwrap();
        let private = fs::read_to_string(format!("{dir}/id")).unwrap();
        let tcp = std::net::TcpStream::connect("127.0.0.1:22").unwrap();
        let mut session = ssh2::Session::new().unwrap();
        session.set_tcp_stream(tcp);
        session.handshake().unwrap();
        session
            .userauth_pubkey_memory(&user, None, &private, None)
            .unwrap();
        assert!(session.authenticated());
    }

    #[test]
    #[ignore]
    fn write_spike_key() {
        let dir = std::env::var("GPUTERM_TEST_SSH_KEYDIR").unwrap();
        let key = generate("gputerm:spike").unwrap();
        fs::write(format!("{dir}/id"), key.private_openssh.as_bytes()).unwrap();
        fs::write(format!("{dir}/id.pub"), format!("{}\n", key.public_line)).unwrap();
        fs::write(
            format!("{dir}/install.sh"),
            posix_install_script(&key.public_line),
        )
        .unwrap();
        fs::write(
            format!("{dir}/remove.sh"),
            posix_remove_script(&key.public_line),
        )
        .unwrap();
        let exported = render_export(&key.private_openssh, "spike passphrase").unwrap();
        fs::write(format!("{dir}/exported"), exported.as_bytes()).unwrap();
    }
}
