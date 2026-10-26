//! Unlocking the credential vault with Touch ID (macOS) or Windows Hello
//! (Windows) instead of typing the master password.
//!
//! Enabling it copies the vault's 32-byte derived key, which only exists
//! while the vault is unlocked, into an operating-system protected store.
//! Unlocking asks the platform to verify the user first and only then reads
//! the key back; the vault then checks it by decrypting `credentials.enc`,
//! exactly as a master-password unlock does. Turning the feature off,
//! resetting the vault or creating a new one deletes the stored key.
//!
//! Other platforms report the feature as unavailable.

use std::sync::Arc;

use serde::Serialize;
use tauri::State;
use zeroize::Zeroizing;

use super::credentials::CredentialVaultStatus;
use super::session::AppState;

pub const VAULT_KEY_BYTES: usize = 32;
pub type VaultKey = Zeroizing<[u8; VAULT_KEY_BYTES]>;

/// Shown by the platform's prompt.
pub const UNLOCK_REASON: &str = "Unlock GpuTerm's credential vault";

/// Where the vault key is kept between unlocks, behind the platform's
/// biometric check. A fake implements it in tests so they never touch the
/// real Keychain or DPAPI.
pub trait BiometricKeyStore: Send + Sync {
    /// The method's name (`Touch ID`, `Windows Hello`) when this machine can
    /// use it now; None when it has no sensor or none is set up.
    fn kind(&self) -> Option<&'static str>;
    /// Whether a key is stored. Must not prompt the user.
    fn is_enabled(&self) -> bool;
    fn save_key(&self, key: &[u8; VAULT_KEY_BYTES]) -> Result<(), String>;
    /// Removing a key that is not there succeeds.
    fn delete_key(&self) -> Result<(), String>;
    /// Asks the user to verify, then returns the stored key.
    fn unlock_key(&self, reason: &str) -> Result<VaultKey, String>;
}

/// For platforms without a supported method, and for tests that must not
/// reach the real store.
#[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
pub struct UnsupportedBiometrics;

impl BiometricKeyStore for UnsupportedBiometrics {
    fn kind(&self) -> Option<&'static str> {
        None
    }

    fn is_enabled(&self) -> bool {
        false
    }

    fn save_key(&self, _key: &[u8; VAULT_KEY_BYTES]) -> Result<(), String> {
        Err(unavailable_error())
    }

    fn delete_key(&self) -> Result<(), String> {
        Ok(())
    }

    fn unlock_key(&self, _reason: &str) -> Result<VaultKey, String> {
        Err(unavailable_error())
    }
}

#[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
fn unavailable_error() -> String {
    "Biometric unlock is not available on this computer".to_string()
}

/// The store for the platform GpuTerm runs on.
pub fn platform_store() -> Arc<dyn BiometricKeyStore> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(macos::TouchIdStore::new())
    }
    #[cfg(windows)]
    {
        Arc::new(windows_hello::WindowsHelloStore::new())
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Arc::new(UnsupportedBiometrics)
    }
}

pub(crate) fn key_from_bytes(bytes: &[u8]) -> Result<VaultKey, String> {
    if bytes.len() != VAULT_KEY_BYTES {
        return Err("The stored vault key is damaged".to_string());
    }
    let mut key = Zeroizing::new([0_u8; VAULT_KEY_BYTES]);
    key.copy_from_slice(bytes);
    Ok(key)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BiometricStatus {
    pub available: bool,
    pub enabled: bool,
    pub kind: Option<String>,
}

#[tauri::command]
pub async fn biometric_status(state: State<'_, AppState>) -> Result<BiometricStatus, String> {
    let credentials = state.credentials.clone();
    tauri::async_runtime::spawn_blocking(move || credentials.biometric_status())
        .await
        .map_err(|error| format!("Biometric status task failed: {error}"))
}

#[tauri::command]
pub async fn enable_biometric_unlock(
    state: State<'_, AppState>,
) -> Result<BiometricStatus, String> {
    let credentials = state.credentials.clone();
    tauri::async_runtime::spawn_blocking(move || {
        credentials.enable_biometric_unlock()?;
        Ok(credentials.biometric_status())
    })
    .await
    .map_err(|error| format!("Biometric setup task failed: {error}"))?
}

#[tauri::command]
pub async fn disable_biometric_unlock(
    state: State<'_, AppState>,
) -> Result<BiometricStatus, String> {
    let credentials = state.credentials.clone();
    tauri::async_runtime::spawn_blocking(move || {
        credentials.disable_biometric_unlock()?;
        Ok(credentials.biometric_status())
    })
    .await
    .map_err(|error| format!("Biometric setup task failed: {error}"))?
}

#[tauri::command]
pub async fn unlock_credential_vault_biometric(
    state: State<'_, AppState>,
) -> Result<CredentialVaultStatus, String> {
    let credentials = state.credentials.clone();
    // The platform prompt blocks this thread until the user answers.
    tauri::async_runtime::spawn_blocking(move || credentials.unlock_with_biometrics())
        .await
        .map_err(|error| format!("Biometric unlock task failed: {error}"))?
}

#[cfg(target_os = "macos")]
mod macos {
    //! Touch ID through LocalAuthentication, with the key in the login
    //! Keychain as a generic password.
    //!
    //! GpuTerm is ad-hoc signed, so it cannot use the data-protection
    //! Keychain with a biometric access-control flag (that needs a
    //! keychain-access-groups entitlement and a team identity). The key is
    //! therefore an ordinary login-Keychain item that only GpuTerm reads,
    //! and GpuTerm reads it only after `evaluatePolicy` has succeeded: the
    //! biometric check gates the read.

    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::time::Duration;

    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::{NSError, NSString};
    use objc2_local_authentication::{LABiometryType, LAContext, LAPolicy};
    use security_framework::passwords::{
        delete_generic_password, get_generic_password, set_generic_password,
    };
    use zeroize::Zeroizing;

    use super::{key_from_bytes, BiometricKeyStore, VaultKey, VAULT_KEY_BYTES};
    use crate::ssh::session::config_dir;

    const SERVICE: &str = "com.gputerm.desktop.vault";
    const ACCOUNT: &str = "vault-key";
    /// errSecItemNotFound.
    const ITEM_NOT_FOUND: i32 = -25300;
    /// LAError codes for a prompt the user or the system dismissed.
    const LA_USER_CANCEL: isize = -2;
    const LA_USER_FALLBACK: isize = -3;
    const LA_SYSTEM_CANCEL: isize = -4;
    const LA_APP_CANCEL: isize = -9;
    /// The prompt stays up while the user decides; this only bounds a
    /// reply that never comes.
    const PROMPT_TIMEOUT: Duration = Duration::from_secs(180);

    pub struct TouchIdStore {
        /// A non-secret marker that a key is stored, so the status check
        /// never has to read the Keychain item.
        marker: PathBuf,
    }

    impl TouchIdStore {
        pub fn new() -> Self {
            Self {
                marker: config_dir().join("vault_key.touchid"),
            }
        }
    }

    fn evaluate(reason: &str) -> Result<(), String> {
        let context = unsafe { LAContext::new() };
        let (sender, receiver) = mpsc::channel::<Result<(), String>>();
        let reply = RcBlock::new(move |success: Bool, error: *mut NSError| {
            let result = if success.as_bool() {
                Ok(())
            } else {
                // SAFETY: LocalAuthentication passes nil or a valid NSError.
                let error = unsafe { error.as_ref() };
                Err(match error.map(|error| error.code()) {
                    Some(LA_USER_CANCEL | LA_SYSTEM_CANCEL | LA_APP_CANCEL) => {
                        "Touch ID was cancelled".to_string()
                    }
                    Some(LA_USER_FALLBACK) => "Use the master password instead".to_string(),
                    _ => error
                        .map(|error| error.localizedDescription().to_string())
                        .unwrap_or_else(|| "Touch ID did not verify you".to_string()),
                })
            };
            let _ = sender.send(result);
        });
        let reason = NSString::from_str(reason);
        // SAFETY: the reply only sends on a channel, which is thread-safe,
        // and the context stays alive until the reply arrives below.
        unsafe {
            context.evaluatePolicy_localizedReason_reply(
                LAPolicy::DeviceOwnerAuthenticationWithBiometrics,
                &reason,
                &reply,
            );
        }
        let result = receiver
            .recv_timeout(PROMPT_TIMEOUT)
            .unwrap_or_else(|_| Err("Touch ID did not answer".to_string()));
        drop(context);
        result
    }

    impl BiometricKeyStore for TouchIdStore {
        fn kind(&self) -> Option<&'static str> {
            let context = unsafe { LAContext::new() };
            unsafe {
                context.canEvaluatePolicy_error(LAPolicy::DeviceOwnerAuthenticationWithBiometrics)
            }
            .ok()?;
            match unsafe { context.biometryType() } {
                LABiometryType::None => None,
                // A Mac's only biometry is Touch ID.
                _ => Some("Touch ID"),
            }
        }

        fn is_enabled(&self) -> bool {
            self.marker.exists()
        }

        fn save_key(&self, key: &[u8; VAULT_KEY_BYTES]) -> Result<(), String> {
            set_generic_password(SERVICE, ACCOUNT, key)
                .map_err(|error| format!("The key could not be saved in the Keychain: {error}"))?;
            if let Err(error) = crate::ssh::managed_key::write_private_file(&self.marker, b"1\n") {
                let _ = delete_generic_password(SERVICE, ACCOUNT);
                return Err(error);
            }
            Ok(())
        }

        fn delete_key(&self) -> Result<(), String> {
            let keychain = match delete_generic_password(SERVICE, ACCOUNT) {
                Ok(()) => Ok(()),
                Err(error) if error.code() == ITEM_NOT_FOUND => Ok(()),
                Err(error) => Err(format!(
                    "The key could not be removed from the Keychain: {error}"
                )),
            };
            match std::fs::remove_file(&self.marker) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("Could not turn off Touch ID unlock: {error}")),
            }
            keychain
        }

        fn unlock_key(&self, reason: &str) -> Result<VaultKey, String> {
            evaluate(reason)?;
            let bytes =
                Zeroizing::new(get_generic_password(SERVICE, ACCOUNT).map_err(|error| {
                    if error.code() == ITEM_NOT_FOUND {
                        "The Touch ID key is no longer in the Keychain".to_string()
                    } else {
                        format!("The Keychain did not return the key: {error}")
                    }
                })?);
            key_from_bytes(&bytes)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Asks LocalAuthentication whether Touch ID can be used, without
        /// prompting. Run by hand: `cargo test -- --ignored touch_id`.
        #[test]
        #[ignore = "queries this Mac's LocalAuthentication"]
        fn touch_id_availability_can_be_queried() {
            let kind = TouchIdStore {
                marker: std::env::temp_dir().join("gputerm-touchid-marker-test"),
            }
            .kind();
            eprintln!("Touch ID available: {kind:?}");
        }
    }
}

#[cfg(windows)]
mod windows_hello {
    //! Windows Hello through UserConsentVerifier, with the key protected by
    //! DPAPI for the current user in `vault_key.dpapi`. DPAPI ties the file
    //! to this Windows account; GpuTerm decrypts it only after Windows Hello
    //! has verified the user.

    use std::path::PathBuf;

    use windows::core::HSTRING;
    use windows::Security::Credentials::UI::{
        UserConsentVerificationResult, UserConsentVerifier, UserConsentVerifierAvailability,
    };
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    use zeroize::Zeroizing;

    use super::{key_from_bytes, BiometricKeyStore, VaultKey, VAULT_KEY_BYTES};
    use crate::ssh::session::config_dir;

    pub struct WindowsHelloStore {
        path: PathBuf,
    }

    impl WindowsHelloStore {
        pub fn new() -> Self {
            Self {
                path: config_dir().join("vault_key.dpapi"),
            }
        }
    }

    /// Copies a DPAPI output blob, wipes it and frees it.
    fn take_blob(blob: CRYPT_INTEGER_BLOB) -> Zeroizing<Vec<u8>> {
        if blob.pbData.is_null() {
            return Zeroizing::new(Vec::new());
        }
        // SAFETY: DPAPI returned `cbData` bytes at `pbData`, allocated with
        // LocalAlloc, which LocalFree releases.
        unsafe {
            let bytes = Zeroizing::new(
                std::slice::from_raw_parts(blob.pbData, blob.cbData as usize).to_vec(),
            );
            std::ptr::write_bytes(blob.pbData, 0, blob.cbData as usize);
            let _ = LocalFree(Some(HLOCAL(blob.pbData.cast())));
            bytes
        }
    }

    fn protect(data: &[u8]) -> Result<Zeroizing<Vec<u8>>, String> {
        let input = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB::default();
        let description = HSTRING::from("GpuTerm vault key");
        // SAFETY: `input` points at `data`, which outlives the call; DPAPI
        // only reads it.
        unsafe {
            CryptProtectData(
                &input,
                &description,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
        .map_err(|error| format!("Windows could not protect the vault key: {error}"))?;
        Ok(take_blob(output))
    }

    fn unprotect(data: &[u8]) -> Result<Zeroizing<Vec<u8>>, String> {
        let input = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB::default();
        // SAFETY: as in `protect`.
        unsafe {
            CryptUnprotectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
        .map_err(|error| format!("Windows could not open the stored vault key: {error}"))?;
        Ok(take_blob(output))
    }

    fn verify(reason: &str) -> Result<(), String> {
        let result = UserConsentVerifier::RequestVerificationAsync(&HSTRING::from(reason))
            .and_then(|operation| operation.get())
            .map_err(|error| format!("Windows Hello is unavailable: {error}"))?;
        if result == UserConsentVerificationResult::Verified {
            Ok(())
        } else if result == UserConsentVerificationResult::Canceled {
            Err("Windows Hello was cancelled".to_string())
        } else {
            Err("Windows Hello did not verify you".to_string())
        }
    }

    impl BiometricKeyStore for WindowsHelloStore {
        fn kind(&self) -> Option<&'static str> {
            UserConsentVerifier::CheckAvailabilityAsync()
                .and_then(|operation| operation.get())
                .ok()
                .filter(|availability| *availability == UserConsentVerifierAvailability::Available)
                .map(|_| "Windows Hello")
        }

        fn is_enabled(&self) -> bool {
            self.path.exists()
        }

        fn save_key(&self, key: &[u8; VAULT_KEY_BYTES]) -> Result<(), String> {
            let protected = protect(key)?;
            crate::ssh::managed_key::write_private_file(&self.path, &protected)
        }

        fn delete_key(&self) -> Result<(), String> {
            match std::fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(format!("Could not turn off Windows Hello unlock: {error}")),
            }
        }

        fn unlock_key(&self, reason: &str) -> Result<VaultKey, String> {
            verify(reason)?;
            let protected = std::fs::read(&self.path).map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    "The Windows Hello key is missing".to_string()
                } else {
                    format!("Could not read the Windows Hello key: {error}")
                }
            })?;
            let key = unprotect(&protected)?;
            key_from_bytes(&key)
        }
    }
}
