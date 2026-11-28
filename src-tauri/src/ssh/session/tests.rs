use super::known_hosts::{evaluate_host_key, HostKeyDecision, KnownHostEntry, KnownHosts};
use super::tunnel::{
    pump_socket_and_channel, ChannelIo, SocketIo, LIBSSH2_ERROR_CHANNEL_FAILURE,
    LIBSSH2_ERROR_CHANNEL_REQUEST_DENIED, LIBSSH2_ERROR_EAGAIN,
};
use super::*;
use crate::ssh::credentials::MemoryCredentialStore;
use std::io::ErrorKind;
use std::sync::atomic::AtomicBool;

fn per_type(pairs: &[(&str, &str)]) -> KnownHostEntry {
    KnownHostEntry::PerType(
        pairs
            .iter()
            .map(|(key_type, fp)| (key_type.to_string(), fp.to_string()))
            .collect(),
    )
}

#[test]
fn host_key_sentinels_are_reworded_for_background_callers() {
    let changed = format!("{}aa|bb|ssh-ed25519|gpu.lab:22", HOST_KEY_CHANGED_PREFIX);
    let unknown = format!("{}bb|ssh-ed25519|gpu.lab:22", UNKNOWN_HOST_KEY_PREFIX);
    assert!(is_host_key_sentinel(&changed));
    assert!(is_host_key_sentinel(&unknown));
    assert!(!is_host_key_sentinel("Authentication failed"));

    let message = readable_connect_error(&changed);
    assert!(
        message.contains("gpu.lab:22") && message.contains("changed"),
        "{message}"
    );
    assert!(!message.contains(HOST_KEY_CHANGED_PREFIX));
    let message = readable_connect_error(&unknown);
    assert!(message.contains("gpu.lab:22"), "{message}");
    assert!(!message.contains(UNKNOWN_HOST_KEY_PREFIX));
    assert_eq!(readable_connect_error("timed out"), "timed out");
}

#[test]
fn formats_ipv4_names_and_ipv6_addresses() {
    assert_eq!(format_host_port("example.com", 22), "example.com:22");
    assert_eq!(format_host_port("2001:db8::1", 2222), "[2001:db8::1]:2222");
    assert_eq!(format_host_port("[2001:db8::1]", 22), "[2001:db8::1]:22");
    assert_eq!(host_without_brackets("[2001:db8::1]"), "2001:db8::1");
}

#[test]
fn the_eagain_constant_matches_what_ssh2_itself_calls_would_block() {
    // `open_direct_channel`'s retry loop hinges on this number. If ssh2 ever
    // changes it, a tunnel would bind a port and silently never serve, so
    // pin it against ssh2's own io::ErrorKind mapping rather than trusting
    // the literal.
    let eagain = ssh2::Error::from_errno(ssh2::ErrorCode::Session(LIBSSH2_ERROR_EAGAIN));
    assert_eq!(
        std::io::Error::from(ssh2::Error::from_errno(ssh2::ErrorCode::Session(
            LIBSSH2_ERROR_EAGAIN
        )))
        .kind(),
        ErrorKind::WouldBlock
    );
    assert!(is_would_block(&eagain));

    // A real failure must not be mistaken for backpressure, or the loop
    // would spin until its deadline instead of reporting the refusal.
    let refused = ssh2::Error::from_errno(ssh2::ErrorCode::Session(-32));
    assert!(!is_would_block(&refused));
}

#[test]
fn a_signature_ignores_the_password_but_tracks_every_hop() {
    let base = SshTarget {
        session_id: "s".to_string(),
        host: "Host.Example".to_string(),
        port: 22,
        username: "user".to_string(),
        password: Some("hunter2".to_string()),
        private_key_path: None,
        managed_key: None,
        assigned_key: None,
        proxy: None,
    };
    let signature = target_signature(&base);
    // Unlocking the vault must not tear down a healthy connection.
    assert!(!signature.contains("hunter2"), "{signature}");
    assert!(signature.contains("host.example:22"), "{signature}");

    let mut without_password = base.clone();
    without_password.password = None;
    assert_eq!(target_signature(&without_password), signature);

    // Anything that changes where the connection lands must change it.
    let mut other_port = base.clone();
    other_port.port = 2222;
    assert_ne!(target_signature(&other_port), signature);

    let mut with_key = base.clone();
    with_key.private_key_path = Some("/keys/id_ed25519".to_string());
    assert_ne!(target_signature(&with_key), signature);

    let mut jumped = base.clone();
    jumped.proxy = Some(Box::new(SshTarget {
        session_id: "b".to_string(),
        host: "bastion".to_string(),
        port: 22,
        username: "jump".to_string(),
        password: Some("also-secret".to_string()),
        private_key_path: None,
        managed_key: None,
        assigned_key: None,
        proxy: None,
    }));
    let jumped_signature = target_signature(&jumped);
    assert_ne!(jumped_signature, signature);
    assert!(!jumped_signature.contains("also-secret"), "{jumped_signature}");
}

#[test]
fn trusts_matching_per_type_fingerprint() {
    let entry = per_type(&[("ssh-ed25519", "abc123")]);
    assert_eq!(
        evaluate_host_key(Some(&entry), "ssh-ed25519", "abc123"),
        HostKeyDecision::Trusted
    );
}

#[test]
fn rejects_differing_fingerprint_for_same_key_type() {
    let entry = per_type(&[("ssh-ed25519", "abc123")]);
    assert_eq!(
        evaluate_host_key(Some(&entry), "ssh-ed25519", "def456"),
        HostKeyDecision::Mismatch {
            expected: "abc123".to_string()
        }
    );
}

#[test]
fn treats_new_key_type_as_unknown_not_mismatch() {
    // The RSA -> ECDSA backend-switch incident: same host, key type never
    // recorded before, must prompt instead of blocking.
    let entry = per_type(&[("ssh-rsa", "abc123")]);
    assert_eq!(
        evaluate_host_key(Some(&entry), "ecdsa-sha2-nistp256", "def456"),
        HostKeyDecision::Unknown
    );
}

#[test]
fn migrates_matching_legacy_fingerprint() {
    let entry = KnownHostEntry::Legacy("abc123".to_string());
    assert_eq!(
        evaluate_host_key(Some(&entry), "ssh-rsa", "abc123"),
        HostKeyDecision::TrustedMigrateLegacy
    );
}

#[test]
fn treats_differing_legacy_fingerprint_as_unknown() {
    let entry = KnownHostEntry::Legacy("abc123".to_string());
    assert_eq!(
        evaluate_host_key(Some(&entry), "ecdsa-sha2-nistp256", "def456"),
        HostKeyDecision::Unknown
    );
}

#[test]
fn treats_missing_entry_as_unknown() {
    assert_eq!(
        evaluate_host_key(None, "ssh-ed25519", "abc123"),
        HostKeyDecision::Unknown
    );
}

fn profile(id: &str, proxy: Option<&str>) -> SessionProfile {
    SessionProfile {
        id: id.to_string(),
        name: id.to_string(),
        host: format!("{}.example", id),
        port: 22,
        username: "user".to_string(),
        is_local: false,
        private_key_path: None,
        proxy_jump_id: proxy.map(str::to_string),
        forwards: Vec::new(),
        managed_key: None,
    }
}

#[test]
fn sign_in_methods_follow_the_credential_and_what_the_server_offers() {
    use AuthMethod::*;
    let order = |offered: &str, key: bool, password: bool| {
        let mut tried = Vec::new();
        while let Some(method) = next_auth_method(offered, false, key, password, &tried, false) {
            tried.push(method);
        }
        tried
    };
    // A key, then a code when the server also asks for one.
    assert_eq!(
        order("publickey,keyboard-interactive", true, true),
        [PublicKey, KeyboardInteractive]
    );
    assert_eq!(order("publickey", true, false), [PublicKey]);
    // A password goes through keyboard-interactive when that is all there is.
    assert_eq!(
        order("publickey,keyboard-interactive", false, true),
        [KeyboardInteractive]
    );
    assert_eq!(order("publickey,password", false, true), [Password]);
    assert_eq!(
        order("password,keyboard-interactive", false, true),
        [Password, KeyboardInteractive]
    );
    // Nothing configured: the agent, then questions.
    assert_eq!(
        order("publickey,keyboard-interactive", false, false),
        [Agent, KeyboardInteractive]
    );
    // An unknown list keeps the old single attempt.
    assert_eq!(order("", false, true), [Password]);
    assert_eq!(order("", false, false), [Agent]);
}

#[test]
fn a_managed_key_goes_first_and_the_password_still_works_after_it() {
    use AuthMethod::*;
    let order = |offered: &str, key: bool, password: bool| {
        let mut tried = Vec::new();
        while let Some(method) = next_auth_method(offered, true, key, password, &tried, false) {
            tried.push(method);
        }
        tried
    };
    assert_eq!(order("publickey,password", false, true), [ManagedKey, Password]);
    assert_eq!(
        order("publickey,keyboard-interactive", false, false),
        [ManagedKey, Agent, KeyboardInteractive]
    );
    assert_eq!(order("publickey", true, false), [ManagedKey, PublicKey]);
}

#[test]
fn managed_key_verification_never_tries_other_credentials() {
    use AuthMethod::*;
    for offered in ["", "publickey,password", "keyboard-interactive", "password"] {
        assert_eq!(
            next_auth_method(offered, true, true, true, &[], true),
            Some(ManagedKey)
        );
        assert_eq!(
            next_auth_method(offered, true, true, true, &[ManagedKey], true),
            None
        );
        assert_eq!(
            next_auth_method(offered, false, true, true, &[], true),
            None
        );
    }
}

/// Live check of the whole sign-in path with a vault-style key: needs
/// `GPUTERM_TEST_SSH_USER` and `GPUTERM_TEST_SSH_KEYDIR/id` trusted by the
/// local sshd (see managed_key's `write_spike_key`).
#[test]
#[ignore]
fn authenticate_signs_in_with_a_managed_key() {
    let user = std::env::var("GPUTERM_TEST_SSH_USER").unwrap();
    let dir = std::env::var("GPUTERM_TEST_SSH_KEYDIR").unwrap();
    let private = std::fs::read_to_string(format!("{dir}/id")).unwrap();
    let key = crate::ssh::managed_key::SecretKey::from_openssh(zeroize::Zeroizing::new(private))
        .unwrap();
    let target = SshTarget {
        session_id: "local-test".into(),
        host: "127.0.0.1".into(),
        port: 22,
        username: user,
        password: None,
        private_key_path: None,
        managed_key: Some(key),
        assigned_key: None,
        proxy: None,
    };
    let tcp = TcpStream::connect("127.0.0.1:22").unwrap();
    let mut session = Session::new().unwrap();
    session.set_tcp_stream(tcp);
    session.handshake().unwrap();
    authenticate(&session, &target, &mut NonInteractive::default(), false).unwrap();
    assert!(session.authenticated());
}

#[test]
fn the_interactive_sign_in_sentinel_is_reworded_for_background_errors() {
    let hop = HopInfo {
        session_id: "s".into(),
        host: "lab".into(),
        port: 22,
        username: "alice".into(),
        saved_password: None,
    };
    let sentinel = interactive_auth_sentinel(&hop);
    assert_eq!(sentinel, "INTERACTIVE_AUTH_REQUIRED:alice@lab:22");
    assert!(readable_connect_error(&sentinel)
        .starts_with("alice@lab:22 asks for a password or one-time code"));
}

#[test]
fn a_refused_probe_channel_means_the_server_answered() {
    let error = |code: i32| ssh2::Error::new(ssh2::ErrorCode::Session(code), "test");
    assert_eq!(classify_probe(Ok(())), ProbeOutcome::Alive);
    assert_eq!(
        classify_probe(Err(&error(LIBSSH2_ERROR_CHANNEL_FAILURE))),
        ProbeOutcome::Alive
    );
    assert_eq!(
        classify_probe(Err(&error(LIBSSH2_ERROR_CHANNEL_REQUEST_DENIED))),
        ProbeOutcome::Alive
    );
    assert_eq!(
        classify_probe(Err(&error(LIBSSH2_ERROR_EAGAIN))),
        ProbeOutcome::Pending
    );
    // Socket send/receive failures and a disconnect mean the transport
    // is gone.
    for code in [-7, -43, -13] {
        assert!(matches!(
            classify_probe(Err(&error(code))),
            ProbeOutcome::Dead(_)
        ));
    }
}

#[test]
fn idle_backoff_doubles_to_its_cap_and_resets_after_activity() {
    let mut idle = IdleBackoff::new(Duration::from_millis(1), Duration::from_millis(4));
    assert_eq!(idle.next_delay(), Duration::from_millis(1));
    idle.sleep();
    assert_eq!(idle.next_delay(), Duration::from_millis(2));
    idle.sleep();
    idle.sleep();
    idle.sleep();
    assert_eq!(idle.next_delay(), Duration::from_millis(4));
    idle.reset();
    assert_eq!(idle.next_delay(), Duration::from_millis(1));
}

#[test]
fn connection_slots_stop_at_the_limit_and_free_on_drop() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let counter = Arc::new(AtomicUsize::new(0));
    let first = try_reserve_slot(&counter, 2).expect("first slot");
    let _second = try_reserve_slot(&counter, 2).expect("second slot");
    assert!(try_reserve_slot(&counter, 2).is_none());
    assert_eq!(counter.load(Ordering::SeqCst), 2);
    drop(first);
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    assert!(try_reserve_slot(&counter, 2).is_some());
}

struct LockedVault;
impl CredentialStore for LockedVault {
    fn set_password(&self, _: &str, _: String) -> Result<(), String> {
        Err("vault locked".to_string())
    }
    fn get_password(&self, _: &str) -> Result<Option<String>, String> {
        Ok(None)
    }
    fn clear_password(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

fn save_with(
    credentials: &impl CredentialStore,
    profile: SessionProfile,
    password: Option<&str>,
    jump_password: Option<&str>,
) -> Result<Vec<SessionProfile>, String> {
    save_profile_and_credentials(
        profile,
        password.map(str::to_string),
        jump_password.map(str::to_string),
        credentials,
        |profile| Ok(vec![profile]),
    )
}

#[test]
fn saving_a_profile_keeps_a_stored_password_unless_a_new_one_is_typed() {
    let credentials = MemoryCredentialStore::default();
    credentials.set_password("gpu", "old".to_string()).unwrap();

    save_with(&credentials, profile("gpu", None), None, None).unwrap();
    assert_eq!(credentials.get_password("gpu").unwrap().as_deref(), Some("old"));
    save_with(&credentials, profile("gpu", None), Some(""), None).unwrap();
    assert_eq!(credentials.get_password("gpu").unwrap().as_deref(), Some("old"));

    save_with(&credentials, profile("gpu", Some("jump")), Some("new"), Some("hop")).unwrap();
    assert_eq!(credentials.get_password("gpu").unwrap().as_deref(), Some("new"));
    assert_eq!(credentials.get_password("jump").unwrap().as_deref(), Some("hop"));
}

#[test]
fn a_local_profile_never_stores_a_password() {
    let credentials = MemoryCredentialStore::default();
    let mut local = profile("laptop", None);
    local.is_local = true;
    save_with(&credentials, local, Some("ignored"), None).unwrap();
    assert_eq!(credentials.get_password("laptop").unwrap(), None);
}

#[test]
fn a_locked_vault_fails_the_save_before_the_profile_is_written() {
    let mut written = false;
    let result = save_profile_and_credentials(
        profile("gpu", None),
        Some("pw".to_string()),
        None,
        &LockedVault,
        |profile| {
            written = true;
            Ok(vec![profile])
        },
    );
    assert_eq!(result.unwrap_err(), "vault locked");
    assert!(!written);
}

#[test]
fn merging_an_edited_profile_keeps_its_forward_rules() {
    let mut original = profile("gpu", None);
    original.forwards = vec![crate::ssh::forwarding::ForwardSpec {
        id: "jupyter".to_string(),
        kind: crate::ssh::forwarding::ForwardKind::Local,
        bind_host: "127.0.0.1".to_string(),
        bind_port: 8888,
        dest_host: Some("localhost".to_string()),
        dest_port: Some(8888),
        enabled: true,
    }];
    let mut profiles = vec![original];
    let mut edited = profile("gpu", None);
    edited.host = "new.example".to_string();
    merge_profile(&mut profiles, edited);
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0].host, "new.example");
    assert_eq!(profiles[0].forwards.len(), 1);
}

#[test]
fn refuses_an_edit_that_makes_a_jump_host_loop() {
    let mut profiles = vec![profile("a", None), profile("b", Some("a"))];
    assert!(ensure_no_jump_loop("b", &profiles).is_ok());
    merge_profile(&mut profiles, profile("a", Some("b")));
    assert!(ensure_no_jump_loop("a", &profiles).unwrap_err().contains("loop"));
}

#[test]
fn refuses_to_delete_a_profile_used_as_a_jump_host_or_by_db_monitor() {
    let profiles = vec![profile("jump", None), profile("gpu", Some("jump"))];
    let error = ensure_profile_unreferenced("jump", &profiles, &[], &[]).unwrap_err();
    assert!(error.contains("gpu"), "{error}");
    assert!(ensure_profile_unreferenced("gpu", &profiles, &[], &[]).is_ok());

    let db = crate::ssh::db_monitor::DbInstance {
        id: "db-1".to_string(),
        name: "prod-pg".to_string(),
        kind: crate::ssh::db_monitor::DbKind::Postgres,
        profile_id: "gpu".to_string(),
        host: "127.0.0.1".to_string(),
        port: 5432,
        user: "postgres".to_string(),
        database: String::new(),
        has_password: false,
    };
    let db_error = ensure_profile_unreferenced("gpu", &profiles, &[], &[db]).unwrap_err();
    assert!(db_error.contains("prod-pg"), "{db_error}");
}

#[test]
fn pump_socket_and_channel_preserves_local_half_close() {
    use std::sync::atomic::{AtomicU64, Ordering};

    struct MockSocket {
        incoming: Vec<u8>,
        written: Vec<u8>,
        write_shutdown: bool,
    }

    impl SocketIo for &mut MockSocket {
        fn set_nonblocking_io(&mut self) -> std::io::Result<()> {
            Ok(())
        }

        fn read_bytes(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.incoming.is_empty() {
                return Ok(0);
            }
            let count = self.incoming.len().min(buf.len());
            buf[..count].copy_from_slice(&self.incoming[..count]);
            self.incoming.drain(..count);
            Ok(count)
        }

        fn write_bytes(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn shutdown_write(&mut self) {
            self.write_shutdown = true;
        }
    }

    struct HalfCloseMockChannel {
        received: Vec<u8>,
        eof_sent: bool,
        response: Vec<u8>,
        closed: bool,
    }

    impl ChannelIo for &mut HalfCloseMockChannel {
        fn read_bytes(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if !self.eof_sent {
                return Err(std::io::Error::new(ErrorKind::WouldBlock, "waiting for eof"));
            }
            if self.response.is_empty() {
                return Ok(0);
            }
            let count = self.response.len().min(buf.len());
            buf[..count].copy_from_slice(&self.response[..count]);
            self.response.drain(..count);
            Ok(count)
        }

        fn write_bytes(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.received.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn is_eof(&self) -> bool {
            self.eof_sent && self.response.is_empty()
        }

        fn send_channel_eof(&mut self) -> Result<(), ErrorKind> {
            self.eof_sent = true;
            Ok(())
        }

        fn close_channel(&mut self) {
            self.closed = true;
        }
    }

    let stop = AtomicBool::new(false);
    let bytes = AtomicU64::new(0);
    let mut socket = MockSocket {
        incoming: b"hello".to_vec(),
        written: Vec::new(),
        write_shutdown: false,
    };
    let mut channel = HalfCloseMockChannel {
        received: Vec::new(),
        eof_sent: false,
        response: b"world".to_vec(),
        closed: false,
    };
    pump_socket_and_channel(&mut socket, &mut channel, &stop, Some(&bytes));

    assert_eq!(channel.received, b"hello");
    assert!(channel.eof_sent);
    assert!(channel.closed);
    assert_eq!(socket.written, b"world");
    assert!(socket.write_shutdown);
    assert_eq!(bytes.load(Ordering::Relaxed), 10);
}

#[test]
fn normalizes_local_profiles_without_ssh_credentials() {
    let mut local = profile("local", Some("jump"));
    local.name.clear();
    local.host = "ignored.example".to_string();
    local.port = 22;
    local.username = "ignored".to_string();
    local.is_local = true;
    local.private_key_path = Some("/tmp/key".to_string());

    let local = normalize_profile(local);
    assert_eq!(local.name, "Local terminal");
    assert_eq!(local.host, "localhost");
    assert_eq!(local.port, 0);
    assert_eq!(local.username, "local");
    assert!(local.private_key_path.is_none());
    assert!(local.proxy_jump_id.is_none());
}

fn no_password(_id: &str) -> Option<String> {
    None
}

#[test]
fn resolves_direct_and_single_and_multi_hop_chains() {
    let profiles = vec![
        profile("target", Some("mid")),
        profile("mid", Some("edge")),
        profile("edge", None),
    ];
    assert!(
        build_proxy_target("edge", &profiles, &mut Vec::new(), &no_password)
            .unwrap()
            .unwrap()
            .proxy
            .is_none()
    );

    let chain = build_proxy_target("mid", &profiles, &mut Vec::new(), &no_password)
        .unwrap()
        .unwrap();
    assert_eq!(chain.host, "mid.example");
    assert_eq!(chain.proxy.as_ref().unwrap().host, "edge.example");
    assert!(chain.password.is_none());
}

#[test]
fn fills_hop_password_from_provider() {
    let profiles = vec![profile("edge", None)];
    let password_for = |id: &str| (id == "edge").then(|| "s3cret".to_string());
    let chain = build_proxy_target("edge", &profiles, &mut Vec::new(), &password_for)
        .unwrap()
        .unwrap();
    assert_eq!(chain.password.as_deref(), Some("s3cret"));
}

#[test]
fn restores_saved_credentials_for_connection_tests() {
    let credentials = MemoryCredentialStore::default();
    credentials
        .set_password("target", "target-secret".to_string())
        .unwrap();
    credentials
        .set_password("jump", "jump-secret".to_string())
        .unwrap();
    let mut target = SshTarget {
        session_id: "target".to_string(),
        host: "target.example".to_string(),
        port: 22,
        username: "user".to_string(),
        password: None,
        private_key_path: None,
        managed_key: None,
        assigned_key: None,
        proxy: Some(Box::new(SshTarget {
            session_id: "jump".to_string(),
            host: "jump.example".to_string(),
            port: 22,
            username: "user".to_string(),
            password: None,
            private_key_path: None,
            managed_key: None,
            assigned_key: None,
            proxy: None,
        })),
    };

    restore_stored_credentials(&mut target, &credentials).unwrap();

    assert_eq!(target.password.as_deref(), Some("target-secret"));
    assert_eq!(
        target.proxy.as_ref().unwrap().password.as_deref(),
        Some("jump-secret")
    );
}

#[test]
fn rejects_missing_cycle_and_too_deep_chains() {
    let missing = vec![profile("a", Some("ghost"))];
    assert!(
        build_proxy_target("a", &missing, &mut Vec::new(), &no_password)
            .unwrap_err()
            .contains("not found")
    );

    let self_cycle = vec![profile("a", Some("a"))];
    assert!(
        build_proxy_target("a", &self_cycle, &mut Vec::new(), &no_password)
            .unwrap_err()
            .contains("loop")
    );

    let mutual = vec![profile("a", Some("b")), profile("b", Some("a"))];
    assert!(
        build_proxy_target("a", &mutual, &mut Vec::new(), &no_password)
            .unwrap_err()
            .contains("loop")
    );

    let deep = vec![
        profile("a", Some("b")),
        profile("b", Some("c")),
        profile("c", Some("d")),
        profile("d", None),
    ];
    assert!(
        build_proxy_target("a", &deep, &mut Vec::new(), &no_password)
            .unwrap_err()
            .contains("too deep")
    );
}

#[test]
fn round_trips_mixed_legacy_and_typed_known_hosts() {
    let json = r#"{
        "old.example:22": "aabbcc",
        "new.example:22": { "ssh-ed25519": "ddeeff", "ssh-rsa": "112233" }
    }"#;
    let parsed: KnownHosts = serde_json::from_str(json).unwrap();
    assert!(matches!(
        parsed.get("old.example:22"),
        Some(KnownHostEntry::Legacy(fp)) if fp == "aabbcc"
    ));
    assert!(matches!(
        parsed.get("new.example:22"),
        Some(KnownHostEntry::PerType(map)) if map.len() == 2
    ));

    let serialized = serde_json::to_string(&parsed).unwrap();
    let reparsed: KnownHosts = serde_json::from_str(&serialized).unwrap();
    assert_eq!(reparsed.len(), 2);
}
