//! Imports hosts from an OpenSSH `ssh_config` file into session profiles.
//!
//! Only what a profile can hold is read: `HostName`, `User`, `Port`, the first
//! `IdentityFile` and `ProxyJump`. Values follow OpenSSH's rule that the first
//! one obtained wins, so `Host *` defaults apply wherever a host left a value
//! unset. Wildcard `Host` patterns are not imported themselves.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use super::session::{list_profiles, upsert_profiles, SessionProfile};

/// Same limit as connecting: a chain deeper than this would not connect.
const MAX_JUMP_HOPS: usize = 3;

/// One concrete `Host` alias with its resolved values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigHost {
    pub alias: String,
    /// `HostName`, or the alias itself when there is none.
    pub host: String,
    pub user: Option<String>,
    pub port: u16,
    pub identity_file: Option<String>,
    pub proxy_jump: Option<String>,
    /// An existing profile with the same host, port and user.
    pub conflict_profile_id: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigPreview {
    pub hosts: Vec<ConfigHost>,
    /// Problems with the file as a whole (unsupported `Include`, `Match`, …).
    pub warnings: Vec<String>,
    /// The file read when no text was supplied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigImportRequest {
    /// The config to import; `None` reads `~/.ssh/config`.
    #[serde(default)]
    pub text: Option<String>,
    pub aliases: Vec<String>,
    /// Used for hosts with no `User`.
    #[serde(default)]
    pub default_user: Option<String>,
    /// `IdentityFile` value -> key path to use. Values not listed are resolved
    /// on this computer (`~` and `%d` expand to the home directory).
    #[serde(default)]
    pub key_paths: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigImportResult {
    pub created: usize,
    pub updated: usize,
    pub warnings: Vec<String>,
    pub profiles: Vec<SessionProfile>,
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
struct Block {
    /// Empty for the lines before the first `Host`, which apply to every host.
    patterns: Vec<String>,
    /// Keyword (lower case) and its arguments, unquoted.
    settings: Vec<(String, Vec<String>)>,
}

fn tokenize(line: &str) -> Option<(String, Vec<String>)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    // The keyword ends at whitespace or `=`.
    let split = line
        .find(|c: char| c.is_whitespace() || c == '=')
        .unwrap_or(line.len());
    let keyword = line[..split].to_ascii_lowercase();
    let mut rest = line[split..].trim_start();
    if let Some(stripped) = rest.strip_prefix('=') {
        rest = stripped.trim_start();
    }
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut has_token = false;
    for c in rest.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                has_token = true;
            }
            c if c.is_whitespace() && !quoted => {
                if has_token {
                    args.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            c => {
                current.push(c);
                has_token = true;
            }
        }
    }
    if has_token {
        args.push(current);
    }
    Some((keyword, args))
}

fn parse_blocks(text: &str, warnings: &mut Vec<String>) -> Vec<Block> {
    let mut blocks = vec![Block::default()];
    let mut in_match = false;
    for (number, line) in text.lines().enumerate() {
        let Some((keyword, args)) = tokenize(line) else {
            continue;
        };
        match keyword.as_str() {
            "host" => {
                in_match = false;
                blocks.push(Block {
                    patterns: args,
                    settings: Vec::new(),
                });
            }
            "match" => {
                in_match = true;
                warnings.push(format!(
                    "Line {}: Match blocks are not supported and were ignored",
                    number + 1
                ));
            }
            "include" => warnings.push(format!(
                "Line {}: Include is not supported; import the included file separately",
                number + 1
            )),
            _ if in_match => {}
            _ => {
                if let Some(block) = blocks.last_mut() {
                    block.settings.push((keyword, args));
                }
            }
        }
    }
    blocks
}

fn is_pattern(name: &str) -> bool {
    name.contains(['*', '?', '!'])
}

/// `*` and `?` globbing, case-insensitive like OpenSSH host matching.
fn glob(pattern: &str, text: &str) -> bool {
    fn go(pattern: &[char], text: &[char]) -> bool {
        match pattern.split_first() {
            None => text.is_empty(),
            Some(('*', rest)) => (0..=text.len()).any(|skip| go(rest, &text[skip..])),
            Some(('?', rest)) => !text.is_empty() && go(rest, &text[1..]),
            Some((c, rest)) => text
                .split_first()
                .is_some_and(|(t, text)| t.eq_ignore_ascii_case(c) && go(rest, text)),
        }
    }
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    go(&pattern, &text)
}

/// A `Host` line matches when some pattern matches and no negated one does.
fn block_matches(patterns: &[String], alias: &str) -> bool {
    if patterns.is_empty() {
        return true;
    }
    let mut matched = false;
    for pattern in patterns {
        if let Some(negated) = pattern.strip_prefix('!') {
            if glob(negated, alias) {
                return false;
            }
        } else if glob(pattern, alias) {
            matched = true;
        }
    }
    matched
}

/// Parses `text` into one entry per concrete alias, in file order.
pub fn parse(text: &str) -> ConfigPreview {
    let mut warnings = Vec::new();
    let blocks = parse_blocks(text, &mut warnings);

    let mut aliases = Vec::new();
    let mut seen = HashSet::new();
    for block in &blocks {
        for name in &block.patterns {
            if !is_pattern(name) && seen.insert(name.to_lowercase()) {
                aliases.push(name.clone());
            }
        }
    }

    let hosts = aliases
        .into_iter()
        .map(|alias| resolve_host(&blocks, alias))
        .collect();
    ConfigPreview {
        hosts,
        warnings,
        source_path: None,
    }
}

fn resolve_host(blocks: &[Block], alias: String) -> ConfigHost {
    // First value obtained wins.
    let mut settings: BTreeMap<&str, &[String]> = BTreeMap::new();
    for block in blocks
        .iter()
        .filter(|block| block_matches(&block.patterns, &alias))
    {
        for (keyword, args) in &block.settings {
            settings.entry(keyword.as_str()).or_insert(args.as_slice());
        }
    }
    // The first argument; every keyword read here takes exactly one.
    let values: BTreeMap<&str, &str> = settings
        .iter()
        .filter_map(|(keyword, args)| Some((*keyword, args.first()?.as_str())))
        .collect();

    let mut warnings = Vec::new();
    let host = match values.get("hostname") {
        Some(name) if name.contains('%') => {
            warnings.push(format!(
                "HostName {} uses % tokens, which are not expanded",
                name
            ));
            alias.clone()
        }
        Some(name) => name.to_string(),
        None => alias.clone(),
    };
    let port = match values.get("port") {
        None => 22,
        Some(text) => match text.parse::<u16>() {
            Ok(port) if port > 0 => port,
            _ => {
                warnings.push(format!("Port {} is not valid; using 22", text));
                22
            }
        },
    };
    let user = values.get("user").map(|user| user.to_string());
    if user.is_none() {
        warnings.push("No User; the import uses the default user".to_string());
    }
    let identity_file = values
        .get("identityfile")
        .filter(|path| !path.eq_ignore_ascii_case("none"))
        .map(|path| path.to_string());
    let proxy_jump = values
        .get("proxyjump")
        .filter(|value| !value.eq_ignore_ascii_case("none"))
        .map(|value| value.to_string());
    if settings.contains_key("proxycommand") && proxy_jump.is_none() {
        warnings.push("ProxyCommand is not supported; use ProxyJump".to_string());
    }

    ConfigHost {
        alias,
        host,
        user,
        port,
        identity_file,
        proxy_jump,
        conflict_profile_id: None,
        warnings,
    }
}

// ---------------------------------------------------------------------------
// Importing
// ---------------------------------------------------------------------------

/// `[user@]host[:port]` from a ProxyJump hop that is not an alias.
fn parse_hop(hop: &str) -> Option<(Option<String>, String, u16)> {
    let (user, rest) = match hop.rsplit_once('@') {
        Some((user, rest)) => (Some(user.to_string()), rest),
        None => (None, hop),
    };
    let (host, port) = if let Some(inner) = rest.strip_prefix('[') {
        // [IPv6]:port
        let (host, after) = inner.split_once(']')?;
        let port = after.strip_prefix(':').map(str::parse).transpose().ok()?;
        (host.to_string(), port.unwrap_or(22))
    } else {
        match rest.rsplit_once(':') {
            Some((host, port)) => (host.to_string(), port.parse().ok()?),
            None => (rest.to_string(), 22),
        }
    };
    (!host.is_empty()).then_some((user, host, port))
}

fn same_endpoint(profile: &SessionProfile, host: &str, port: u16, user: &str) -> bool {
    profile.host.eq_ignore_ascii_case(host) && profile.port == port && profile.username == user
}

/// Marks the hosts that match a saved profile.
pub fn mark_conflicts(preview: &mut ConfigPreview, existing: &[SessionProfile]) {
    for host in &mut preview.hosts {
        let user = host.user.clone().unwrap_or_default();
        host.conflict_profile_id = existing
            .iter()
            .find(|profile| same_endpoint(profile, &host.host, host.port, &user))
            .map(|profile| profile.id.clone());
    }
}

/// Works out the profiles to save. Pure, so it can be tested without storage.
///
/// Selected hosts that match a saved profile update it in place (keeping its
/// id, so saved passwords stay attached). ProxyJump hops resolve to a host in
/// the file, then to a saved profile, and otherwise become new profiles.
pub fn plan_import(
    preview: &ConfigPreview,
    existing: &[SessionProfile],
    request: &ConfigImportRequest,
) -> Result<(Vec<SessionProfile>, usize, usize, Vec<String>), String> {
    let default_user = request
        .default_user
        .as_deref()
        .map(str::trim)
        .filter(|user| !user.is_empty());
    let by_alias: HashMap<String, &ConfigHost> = preview
        .hosts
        .iter()
        .map(|host| (host.alias.to_lowercase(), host))
        .collect();

    let mut warnings = Vec::new();
    let mut planned: Vec<SessionProfile> = Vec::new();
    let mut alias_ids: HashMap<String, String> = HashMap::new();
    let (mut created, mut updated) = (0, 0);

    // Selected aliases plus every alias a selected host jumps through.
    let mut queue: Vec<String> = request
        .aliases
        .iter()
        .map(|alias| alias.to_lowercase())
        .collect();
    let mut order = Vec::new();
    let mut visited = HashSet::new();
    while let Some(alias) = queue.pop() {
        if !visited.insert(alias.clone()) {
            continue;
        }
        let Some(host) = by_alias.get(&alias) else {
            return Err(format!("{} is not a host in this file", alias));
        };
        if let Some(jump) = &host.proxy_jump {
            for hop in jump.split(',') {
                let hop = hop.trim().to_lowercase();
                if by_alias.contains_key(&hop) {
                    queue.push(hop);
                }
            }
        }
        order.push(alias);
    }

    // Pass 1: one profile per alias, so pass 2 can resolve references in any
    // order.
    for alias in &order {
        let host = by_alias[alias];
        let Some(user) = host.user.as_deref().or(default_user) else {
            return Err(format!("{} has no User; enter a default user", host.alias));
        };
        let conflict = existing
            .iter()
            .find(|profile| same_endpoint(profile, &host.host, host.port, user));
        let mut profile = match conflict {
            Some(existing) => {
                updated += 1;
                existing.clone()
            }
            None => {
                created += 1;
                SessionProfile {
                    id: Uuid::new_v4().to_string(),
                    name: host.alias.clone(),
                    host: host.host.clone(),
                    port: host.port,
                    username: user.to_string(),
                    is_local: false,
                    private_key_path: None,
                    proxy_jump_id: None,
                    forwards: Vec::new(),
                }
            }
        };
        if let Some(identity) = &host.identity_file {
            match request.key_paths.get(identity) {
                Some(path) => profile.private_key_path = Some(path.clone()),
                None if profile.private_key_path.is_none() => warnings.push(format!(
                    "{}: no key file chosen for {}; connect with a password or add the key later",
                    host.alias, identity
                )),
                None => {}
            }
        }
        alias_ids.insert(alias.clone(), profile.id.clone());
        planned.push(profile);
    }

    // Pass 2: ProxyJump. `a,b` reaches b through a, then the target through b.
    let mut hop_profiles: Vec<SessionProfile> = Vec::new();
    for alias in &order {
        let host = by_alias[alias];
        let Some(jump) = &host.proxy_jump else {
            continue;
        };
        let hops: Vec<&str> = jump.split(',').map(str::trim).collect();
        if hops.len() > MAX_JUMP_HOPS {
            warnings.push(format!(
                "{}: ProxyJump has more than {} hops and was not imported",
                host.alias, MAX_JUMP_HOPS
            ));
            continue;
        }
        let mut previous: Option<String> = None;
        let mut chain_ok = true;
        for hop in hops {
            let id = if let Some(id) = alias_ids.get(&hop.to_lowercase()) {
                id.clone()
            } else if let Some((user, hop_host, port)) = parse_hop(hop) {
                let user = user
                    .or(host.user.clone())
                    .or(default_user.map(str::to_string));
                let Some(user) = user else {
                    warnings.push(format!("{}: jump host {} has no user", host.alias, hop));
                    chain_ok = false;
                    break;
                };
                let found = existing
                    .iter()
                    .chain(planned.iter())
                    .chain(hop_profiles.iter())
                    .find(|profile| same_endpoint(profile, &hop_host, port, &user))
                    .map(|profile| profile.id.clone());
                match found {
                    Some(id) => id,
                    None => {
                        created += 1;
                        let profile = SessionProfile {
                            id: Uuid::new_v4().to_string(),
                            name: format!("{}@{}", user, hop_host),
                            host: hop_host,
                            port,
                            username: user,
                            is_local: false,
                            private_key_path: None,
                            proxy_jump_id: previous.clone(),
                            forwards: Vec::new(),
                        };
                        let id = profile.id.clone();
                        hop_profiles.push(profile);
                        id
                    }
                }
            } else {
                warnings.push(format!("{}: could not read jump host {}", host.alias, hop));
                chain_ok = false;
                break;
            };
            previous = Some(id);
        }
        if chain_ok {
            let id = &alias_ids[alias];
            if let Some(profile) = planned.iter_mut().find(|profile| &profile.id == id) {
                if previous.as_deref() == Some(id.as_str()) {
                    warnings.push(format!("{}: ProxyJump points at itself", host.alias));
                } else {
                    profile.proxy_jump_id = previous;
                }
            }
        }
    }

    planned.extend(hop_profiles);
    Ok((planned, created, updated, warnings))
}

// ---------------------------------------------------------------------------
// This computer
// ---------------------------------------------------------------------------

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|value| !value.is_empty()))
        .map(PathBuf::from)
}

fn default_config_path() -> Result<PathBuf, String> {
    home_dir()
        .map(|home| home.join(".ssh").join("config"))
        .ok_or_else(|| "Could not find your home directory".to_string())
}

/// Expands `~` and `%d` (the home directory) the way OpenSSH does for
/// `IdentityFile`; other `%` tokens cannot be resolved without a connection.
fn expand_identity_path(value: &str, home: &Path) -> Option<PathBuf> {
    let value = value.replace("%d", &home.to_string_lossy());
    if value.contains('%') {
        return None;
    }
    if value == "~" {
        return Some(home.to_path_buf());
    }
    if let Some(rest) = value.strip_prefix("~/").or_else(|| value.strip_prefix("~\\")) {
        return Some(home.join(rest));
    }
    let path = PathBuf::from(&value);
    Some(if path.is_absolute() { path } else { home.join(path) })
}

/// The key file an `IdentityFile` value points at, when it exists here.
fn resolve_identity_file(value: &str) -> Option<String> {
    let path = expand_identity_path(value, &home_dir()?)?;
    path.is_file().then(|| path.to_string_lossy().into_owned())
}

fn read_config(text: Option<String>) -> Result<(String, Option<String>), String> {
    match text {
        Some(text) => Ok((text, None)),
        None => {
            let path = default_config_path()?;
            let text = fs::read_to_string(&path)
                .map_err(|error| format!("Failed to read {}: {}", path.display(), error))?;
            Ok((text, Some(path.to_string_lossy().into_owned())))
        }
    }
}

#[tauri::command(async)]
pub fn preview_ssh_config(text: Option<String>) -> Result<ConfigPreview, String> {
    let (text, source_path) = read_config(text)?;
    let mut preview = parse(&text);
    preview.source_path = source_path;
    mark_conflicts(&mut preview, &list_profiles()?);
    for host in &mut preview.hosts {
        if let Some(identity) = &host.identity_file {
            if resolve_identity_file(identity).is_none() {
                host.warnings.push(format!(
                    "Key file {} was not found on this computer",
                    identity
                ));
            }
        }
    }
    Ok(preview)
}

#[tauri::command(async)]
pub fn import_ssh_config(mut request: ConfigImportRequest) -> Result<ConfigImportResult, String> {
    let (text, _) = read_config(request.text.take())?;
    let preview = parse(&text);
    for host in &preview.hosts {
        if let Some(identity) = &host.identity_file {
            if !request.key_paths.contains_key(identity) {
                if let Some(path) = resolve_identity_file(identity) {
                    request.key_paths.insert(identity.clone(), path);
                }
            }
        }
    }
    let existing = list_profiles()?;
    let (planned, created, updated, warnings) = plan_import(&preview, &existing, &request)?;
    let profiles = upsert_profiles(planned)?;
    Ok(ConfigImportResult {
        created,
        updated,
        warnings,
        profiles,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"
# Lab machines
Host bastion
    HostName bastion.example.org
    User alice
    Port 2222

Host gpu1 gpu2
  HostName=10.0.0.21
  IdentityFile "~/.ssh/id lab"
  ProxyJump bastion

Host gpu-*
    User ops

Host *.internal !secret.internal
    ProxyJump carol@jump.example.org:22

Host db.internal
    HostName %h.corp

Match host foo
    User ignored

Host *
    User fallback
    IdentityFile ~/.ssh/id_ed25519
    Include extra.conf
"#;

    fn host<'a>(preview: &'a ConfigPreview, alias: &str) -> &'a ConfigHost {
        preview
            .hosts
            .iter()
            .find(|host| host.alias == alias)
            .unwrap()
    }

    #[test]
    fn concrete_aliases_are_listed_once_in_file_order() {
        let preview = parse(CONFIG);
        let aliases: Vec<&str> = preview
            .hosts
            .iter()
            .map(|host| host.alias.as_str())
            .collect();
        assert_eq!(aliases, ["bastion", "gpu1", "gpu2", "db.internal"]);
        assert!(preview.warnings.iter().any(|w| w.contains("Match")));
        assert!(preview.warnings.iter().any(|w| w.contains("Include")));
    }

    #[test]
    fn first_value_wins_and_wildcards_supply_defaults() {
        let preview = parse(CONFIG);
        let bastion = host(&preview, "bastion");
        assert_eq!(bastion.host, "bastion.example.org");
        assert_eq!(bastion.port, 2222);
        assert_eq!(bastion.user.as_deref(), Some("alice"));
        // From `Host *`.
        assert_eq!(bastion.identity_file.as_deref(), Some("~/.ssh/id_ed25519"));

        let gpu1 = host(&preview, "gpu1");
        assert_eq!(gpu1.host, "10.0.0.21");
        assert_eq!(gpu1.identity_file.as_deref(), Some("~/.ssh/id lab"));
        assert_eq!(gpu1.proxy_jump.as_deref(), Some("bastion"));
        // `Host gpu-*` does not match gpu1; `Host *` does.
        assert_eq!(gpu1.user.as_deref(), Some("fallback"));

        let db = host(&preview, "db.internal");
        assert_eq!(db.host, "db.internal");
        assert!(db.warnings.iter().any(|w| w.contains("% tokens")));
        assert_eq!(db.proxy_jump.as_deref(), Some("carol@jump.example.org:22"));
    }

    #[test]
    fn negated_patterns_exclude_a_host() {
        assert!(block_matches(&["*.internal".into()], "db.internal"));
        assert!(!block_matches(
            &["*.internal".into(), "!secret.internal".into()],
            "secret.internal"
        ));
        assert!(glob("gpu-?", "GPU-1"));
        assert!(!glob("gpu-?", "gpu-12"));
    }

    #[test]
    fn hops_parse_user_host_and_port() {
        assert_eq!(
            parse_hop("bob@j.example:2200"),
            Some((Some("bob".into()), "j.example".into(), 2200))
        );
        assert_eq!(parse_hop("j.example"), Some((None, "j.example".into(), 22)));
        assert_eq!(
            parse_hop("[fe80::1]:22"),
            Some((None, "fe80::1".into(), 22))
        );
        assert_eq!(parse_hop("j:notaport"), None);
    }

    fn request(aliases: &[&str]) -> ConfigImportRequest {
        ConfigImportRequest {
            text: Some(CONFIG.to_string()),
            aliases: aliases.iter().map(|a| a.to_string()).collect(),
            default_user: None,
            key_paths: HashMap::from([("~/.ssh/id lab".to_string(), "/keys/lab".to_string())]),
        }
    }

    #[test]
    fn importing_a_host_brings_its_jump_host_along() {
        let preview = parse(CONFIG);
        let (planned, created, updated, _) =
            plan_import(&preview, &[], &request(&["gpu1"])).unwrap();
        assert_eq!((created, updated), (2, 0));
        let gpu1 = planned.iter().find(|p| p.name == "gpu1").unwrap();
        let bastion = planned.iter().find(|p| p.name == "bastion").unwrap();
        assert_eq!(gpu1.proxy_jump_id.as_deref(), Some(bastion.id.as_str()));
        assert_eq!(gpu1.private_key_path.as_deref(), Some("/keys/lab"));
        assert_eq!(bastion.port, 2222);
    }

    #[test]
    fn a_literal_jump_host_becomes_a_profile_and_existing_ones_update_in_place() {
        let preview = parse(CONFIG);
        let existing = vec![SessionProfile {
            id: "saved".into(),
            name: "My DB".into(),
            host: "DB.internal".into(),
            port: 22,
            username: "fallback".into(),
            is_local: false,
            private_key_path: None,
            proxy_jump_id: None,
            forwards: Vec::new(),
        }];
        let (planned, created, updated, warnings) =
            plan_import(&preview, &existing, &request(&["db.internal"])).unwrap();
        assert_eq!((created, updated), (1, 1));
        let db = planned.iter().find(|p| p.id == "saved").unwrap();
        assert_eq!(db.name, "My DB");
        let jump = planned
            .iter()
            .find(|p| p.host == "jump.example.org")
            .unwrap();
        assert_eq!(jump.username, "carol");
        assert_eq!(db.proxy_jump_id.as_deref(), Some(jump.id.as_str()));
        assert!(warnings.iter().any(|w| w.contains("no key file")));
    }

    #[test]
    fn chains_link_each_hop_through_the_previous_one() {
        let text = "Host t\n HostName t.example\n User u\n ProxyJump a@one,b@two:2022\n";
        let preview = parse(text);
        let request = ConfigImportRequest {
            text: Some(text.to_string()),
            aliases: vec!["t".into()],
            default_user: None,
            key_paths: HashMap::new(),
        };
        let (planned, created, _, _) = plan_import(&preview, &[], &request).unwrap();
        assert_eq!(created, 3);
        let one = planned.iter().find(|p| p.host == "one").unwrap();
        let two = planned.iter().find(|p| p.host == "two").unwrap();
        let target = planned.iter().find(|p| p.host == "t.example").unwrap();
        assert_eq!(one.proxy_jump_id, None);
        assert_eq!(two.proxy_jump_id.as_deref(), Some(one.id.as_str()));
        assert_eq!(target.proxy_jump_id.as_deref(), Some(two.id.as_str()));
    }

    #[test]
    fn hosts_without_a_user_need_the_default() {
        let text = "Host lonely\n HostName 1.2.3.4\n";
        let preview = parse(text);
        let mut request = ConfigImportRequest {
            text: Some(text.to_string()),
            aliases: vec!["lonely".into()],
            default_user: None,
            key_paths: HashMap::new(),
        };
        assert!(plan_import(&preview, &[], &request).is_err());
        request.default_user = Some("ubuntu".into());
        let (planned, ..) = plan_import(&preview, &[], &request).unwrap();
        assert_eq!(planned[0].username, "ubuntu");
    }

    #[test]
    fn a_self_referencing_jump_is_ignored() {
        let text = "Host loop\n User u\n ProxyJump loop\n";
        let preview = parse(text);
        let request = ConfigImportRequest {
            text: Some(text.to_string()),
            aliases: vec!["loop".into()],
            default_user: None,
            key_paths: HashMap::new(),
        };
        let (planned, _, _, warnings) = plan_import(&preview, &[], &request).unwrap();
        assert_eq!(planned[0].proxy_jump_id, None);
        assert!(warnings.iter().any(|w| w.contains("itself")));
    }

    #[test]
    fn identity_paths_expand_the_home_directory() {
        let home = Path::new("/home/me");
        assert_eq!(
            expand_identity_path("~/.ssh/id_ed25519", home),
            Some(PathBuf::from("/home/me/.ssh/id_ed25519"))
        );
        assert_eq!(
            expand_identity_path("%d/.ssh/key", home),
            Some(PathBuf::from("/home/me/.ssh/key"))
        );
        assert_eq!(
            expand_identity_path("/etc/keys/k", home),
            Some(PathBuf::from("/etc/keys/k"))
        );
        assert_eq!(expand_identity_path("~/.ssh/%h", home), None);
    }
}
