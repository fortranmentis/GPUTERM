use std::collections::HashMap;
use std::sync::Mutex;

pub trait CredentialStore: Send + Sync {
    fn set_password(&self, session_id: &str, password: String);
    fn get_password(&self, session_id: &str) -> Option<String>;
    fn clear_password(&self, session_id: &str);
}

#[derive(Default)]
pub struct MemoryCredentialStore {
    passwords: Mutex<HashMap<String, String>>,
}

impl CredentialStore for MemoryCredentialStore {
    fn set_password(&self, session_id: &str, password: String) {
        if let Ok(mut passwords) = self.passwords.lock() {
            passwords.insert(session_id.to_string(), password);
        }
    }

    fn get_password(&self, session_id: &str) -> Option<String> {
        self.passwords
            .lock()
            .ok()
            .and_then(|passwords| passwords.get(session_id).cloned())
    }

    fn clear_password(&self, session_id: &str) {
        if let Ok(mut passwords) = self.passwords.lock() {
            passwords.remove(session_id);
        }
    }
}
