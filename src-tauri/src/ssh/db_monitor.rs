//! PostgreSQL and MySQL/MariaDB status, read with the database's own client
//! (`psql`, `mysql` or `mariadb`) on a server GpuTerm is connected to.
//!
//! Instances are saved in `db_instances.json`; each names the SSH profile it
//! is reached through and where the database listens as seen from that
//! server. Passwords live in the encrypted credential vault (`db:<id>`), so
//! saving one and reading a status both need the vault unlocked. On the
//! server a password only ever travels through the client's stdin into a
//! private temporary file (`PGPASSFILE`, `--defaults-extra-file`) that is
//! removed on exit, never through arguments or the environment, which
//! other users can read in `ps` or /proc. The queries only read statistics.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;
use zeroize::Zeroizing;

use super::credentials::{write_json_file, CredentialStore};
use super::parse_util::split_sections;
use super::resource_details::resolve_remote_os;
use super::session::{config_dir, target_for_active_session, with_ops_session, AppState};
use super::system_monitor::{run_remote_command_with_input, shell_quote, RemoteOs};

static DB_LOCK: Mutex<()> = Mutex::new(());
const STATUS_BUDGET_SECS: u64 = 20;
const OPS_TIMEOUT_MS: u32 = 25_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DbKind {
    Postgres,
    /// MySQL and MariaDB.
    Mysql,
}

impl DbKind {
    fn default_port(self) -> u16 {
        match self {
            DbKind::Postgres => 5432,
            DbKind::Mysql => 3306,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DbInstance {
    pub id: String,
    pub name: String,
    pub kind: DbKind,
    /// The SSH profile the server is reached through; status is read while
    /// that profile is connected.
    pub profile_id: String,
    /// As seen from the SSH server. Empty means the local Unix socket.
    #[serde(default)]
    pub host: String,
    pub port: u16,
    pub user: String,
    /// PostgreSQL only; empty connects to the user's default database.
    #[serde(default)]
    pub database: String,
    /// Whether a password is saved for it on this device.
    #[serde(default)]
    pub has_password: bool,
}

/// What the form sends. `password`: None keeps the saved one, an empty
/// string removes it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DbInstanceInput {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    pub kind: DbKind,
    pub profile_id: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: Option<u16>,
    pub user: String,
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub password: Option<String>,
}

/// One reading. Counters (commits, queries) are totals since the server
/// started; the app turns successive readings into rates.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DbStatus {
    pub instance_id: String,
    pub version: Option<String>,
    pub connections: Option<u64>,
    pub max_connections: Option<u64>,
    /// Sessions running a query (PostgreSQL `active`, MySQL `Threads_running`).
    pub active: Option<u64>,
    /// PostgreSQL only: sessions holding a transaction open while idle.
    pub idle_in_transaction: Option<u64>,
    /// PostgreSQL only: sessions waiting for a lock.
    pub waiting_on_locks: Option<u64>,
    /// PostgreSQL only: the oldest running query, in seconds.
    pub longest_query_secs: Option<u64>,
    pub commits: Option<u64>,
    pub rollbacks: Option<u64>,
    /// MySQL `Questions`.
    pub queries: Option<u64>,
    pub slow_queries: Option<u64>,
    pub cache_hit_percent: Option<f64>,
    pub deadlocks: Option<u64>,
    pub size_bytes: Option<u64>,
    pub uptime_secs: Option<u64>,
    /// Set on a replica: how far it trails its primary.
    pub replication_lag_secs: Option<u64>,
    pub is_replica: bool,
}

pub fn password_account(instance_id: &str) -> String {
    format!("db:{instance_id}")
}

fn instances_path() -> PathBuf {
    config_dir().join("db_instances.json")
}

fn read_instances(path: &Path) -> Result<Vec<DbInstance>, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| format!("The database list is unreadable: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(format!("Could not read the database list: {error}")),
    }
}

fn write_instances(path: &Path, instances: &[DbInstance]) -> Result<(), String> {
    write_json_file(path, &instances, "database list")
}

fn lock_instances() -> Result<std::sync::MutexGuard<'static, ()>, String> {
    DB_LOCK
        .lock()
        .map_err(|_| "The database list is unavailable".to_string())
}

fn list_instances_at(path: &Path) -> Result<Vec<DbInstance>, String> {
    let _guard = lock_instances()?;
    read_instances(path)
}

pub(crate) fn validate_input(input: &DbInstanceInput) -> Result<(), String> {
    let field_ok = |value: &str| !value.chars().any(char::is_control);
    if input.name.trim().is_empty() {
        return Err("Give the database a name".to_string());
    }
    if input.profile_id.trim().is_empty() {
        return Err("Choose the server it runs on".to_string());
    }
    if input.user.trim().is_empty() {
        return Err("Enter the database user".to_string());
    }
    if input.port == Some(0) {
        return Err("Enter a port between 1 and 65535".to_string());
    }
    let host_ok = input
        .host
        .trim()
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || ".-_:[]%".contains(c));
    if !host_ok {
        return Err("The host may only contain letters, digits, '.', '-', '_' and ':'".to_string());
    }
    if ![&input.name, &input.user, &input.database]
        .iter()
        .all(|value| field_ok(value))
    {
        return Err("Names may not contain control characters".to_string());
    }
    Ok(())
}

fn vault_error(error: String, action: &str) -> String {
    if error.contains("locked") {
        format!("Unlock the credential vault to {action}")
    } else {
        error
    }
}

/// Adds or updates an instance; a new password replaces the saved one.
fn save_instance_at(
    path: &Path,
    vault: &dyn CredentialStore,
    input: DbInstanceInput,
) -> Result<Vec<DbInstance>, String> {
    validate_input(&input)?;
    let _guard = lock_instances()?;
    let mut instances = read_instances(path)?;
    let id = input
        .id
        .clone()
        .filter(|id| !id.trim().is_empty())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let previous = instances.iter().find(|instance| instance.id == id).cloned();
    let password = input.password.map(Zeroizing::new);
    let has_password = match password.as_deref().map(String::as_str) {
        Some("") => {
            if previous
                .as_ref()
                .is_some_and(|instance| instance.has_password)
            {
                vault
                    .clear_password(&password_account(&id))
                    .map_err(|error| vault_error(error, "remove the saved password"))?;
            }
            false
        }
        Some(password) => {
            vault
                .set_password(&password_account(&id), password.to_string())
                .map_err(|error| vault_error(error, "save the database password"))?;
            true
        }
        None => previous
            .as_ref()
            .is_some_and(|instance| instance.has_password),
    };
    let instance = DbInstance {
        id: id.clone(),
        name: input.name.trim().to_string(),
        kind: input.kind,
        profile_id: input.profile_id,
        host: input.host.trim().to_string(),
        port: input.port.unwrap_or_else(|| input.kind.default_port()),
        user: input.user.trim().to_string(),
        database: input.database.trim().to_string(),
        has_password,
    };
    match instances.iter_mut().find(|item| item.id == id) {
        Some(existing) => *existing = instance,
        None => instances.push(instance),
    }
    instances.sort_by_key(|instance| instance.name.to_lowercase());
    write_instances(path, &instances)?;
    Ok(instances)
}

fn delete_instance_at(
    path: &Path,
    vault: &dyn CredentialStore,
    instance_id: &str,
) -> Result<Vec<DbInstance>, String> {
    let _guard = lock_instances()?;
    let mut instances = read_instances(path)?;
    if instances
        .iter()
        .any(|instance| instance.id == instance_id && instance.has_password)
    {
        // Removed first, so a database is never forgotten while its
        // password stays behind in the vault.
        vault
            .clear_password(&password_account(instance_id))
            .map_err(|error| vault_error(error, "remove this database and its password"))?;
    }
    instances.retain(|instance| instance.id != instance_id);
    write_instances(path, &instances)?;
    Ok(instances)
}

#[tauri::command(async)]
pub fn list_db_instances() -> Result<Vec<DbInstance>, String> {
    list_instances_at(&instances_path())
}

#[tauri::command]
pub async fn save_db_instance(
    state: State<'_, AppState>,
    input: DbInstanceInput,
) -> Result<Vec<DbInstance>, String> {
    let vault = state.credentials.clone();
    tauri::async_runtime::spawn_blocking(move || save_instance_at(&instances_path(), &vault, input))
        .await
        .map_err(|error| format!("Database save task failed: {error}"))?
}

#[tauri::command]
pub async fn delete_db_instance(
    state: State<'_, AppState>,
    id: String,
) -> Result<Vec<DbInstance>, String> {
    let vault = state.credentials.clone();
    tauri::async_runtime::spawn_blocking(move || delete_instance_at(&instances_path(), &vault, &id))
        .await
        .map_err(|error| format!("Database delete task failed: {error}"))?
}

/// Reads the instance's status over its profile's SSH connection, which
/// must be open.
#[tauri::command]
pub async fn db_status(state: State<'_, AppState>, id: String) -> Result<DbStatus, String> {
    let instance = list_instances_at(&instances_path())?
        .into_iter()
        .find(|instance| instance.id == id)
        .ok_or_else(|| "That database is no longer saved".to_string())?;
    let password = if instance.has_password {
        state
            .credentials
            .get_password(&password_account(&instance.id))
            .map_err(|error| {
                if error.contains("locked") {
                    format!("Unlock the credential vault to read {}", instance.name)
                } else {
                    error
                }
            })?
            .map(Zeroizing::new)
    } else {
        None
    };
    let connected = state
        .active_connections
        .lock()
        .map_err(|_| "Active session state is unavailable".to_string())?
        .get(&instance.profile_id)
        .map(|connection| !connection.profile.is_local);
    match connected {
        None => return Err("Connect to its server to read this database".to_string()),
        Some(false) => {
            return Err("Databases are read through a remote SSH server".to_string());
        }
        Some(true) => {}
    }
    let target = target_for_active_session(&state, &instance.profile_id)
        .map_err(|_| "Connect to its server to read this database".to_string())?;
    let ops = Arc::clone(&state.ops_sessions);
    let os_cache = Arc::clone(&state.remote_os_cache);
    tauri::async_runtime::spawn_blocking(move || {
        let script = status_script(&instance);
        let mut input = Zeroizing::new(
            password
                .as_deref()
                .map(String::as_str)
                .unwrap_or("")
                .to_string(),
        );
        input.push('\n');
        let output = with_ops_session(&ops, &target, OPS_TIMEOUT_MS, |session| {
            if resolve_remote_os(&os_cache, &instance.profile_id, session) == RemoteOs::Windows {
                return Err("Database status is read on Linux and macOS servers".to_string());
            }
            run_remote_command_with_input(
                session,
                &script,
                Some(input.as_bytes()),
                STATUS_BUDGET_SECS,
            )
        })?;
        let mut status = match instance.kind {
            DbKind::Postgres => parse_postgres(&output),
            DbKind::Mysql => parse_mysql(&output),
        }?;
        status.instance_id = instance.id;
        Ok(status)
    })
    .await
    .map_err(|error| format!("Database status task failed: {error}"))?
}

/// Reads the password from stdin into a private file the client reads, so
/// it never appears in an argument list.
const PASSWORD_PREAMBLE: &str =
    "IFS= read -r pw || true\numask 077\nt=$(mktemp) || exit 1\ntrap 'rm -f \"$t\"' EXIT\n";

const POSTGRES_SQL: &str = "SELECT current_setting('server_version'), \
(SELECT count(*) FROM pg_stat_activity WHERE backend_type = 'client backend'), \
current_setting('max_connections'), \
(SELECT count(*) FROM pg_stat_activity WHERE state = 'active' AND pid <> pg_backend_pid()), \
(SELECT count(*) FROM pg_stat_activity WHERE state LIKE 'idle in transaction%'), \
(SELECT count(*) FROM pg_stat_activity WHERE wait_event_type = 'Lock'), \
(SELECT coalesce(max(extract(epoch FROM now() - query_start)), 0)::bigint FROM pg_stat_activity WHERE state = 'active' AND pid <> pg_backend_pid()), \
d.xact_commit, d.xact_rollback, d.blks_hit, d.blks_read, d.deadlocks, pg_database_size(current_database()), \
extract(epoch FROM now() - pg_postmaster_start_time())::bigint, \
pg_is_in_recovery(), \
CASE WHEN pg_is_in_recovery() THEN coalesce(extract(epoch FROM now() - pg_last_xact_replay_timestamp()), 0)::bigint END \
FROM pg_stat_database d WHERE d.datname = current_database()";

const MYSQL_SQL: &str = "SHOW GLOBAL STATUS WHERE Variable_name IN ('Threads_connected','Threads_running','Questions','Slow_queries','Uptime','Com_commit','Com_rollback','Innodb_buffer_pool_read_requests','Innodb_buffer_pool_reads','Innodb_deadlocks'); SHOW GLOBAL VARIABLES WHERE Variable_name IN ('max_connections','version')";

pub(crate) fn status_script(instance: &DbInstance) -> String {
    let host = shell_quote(&instance.host);
    let port = instance.port;
    let user = shell_quote(&instance.user);
    match instance.kind {
        DbKind::Postgres => {
            let database = shell_quote(&instance.database);
            let sql = shell_quote(POSTGRES_SQL);
            format!(
                "{PASSWORD_PREAMBLE}\
                 H={host}; P={port}; U={user}; D={database}\n\
                 if [ -n \"$pw\" ]; then printf '*:%s:*:%s:%s\\n' \"$P\" \"$(printf '%s' \"$U\" | sed 's/[\\\\:]/\\\\&/g')\" \"$(printf '%s' \"$pw\" | sed 's/[\\\\:]/\\\\&/g')\" > \"$t\"; export PGPASSFILE=\"$t\"; fi\n\
                 command -v psql >/dev/null 2>&1 || {{ echo __NO_CLIENT__; exit 0; }}\n\
                 set -- -X -A -t -q -w -F '\t' -p \"$P\" -U \"$U\"\n\
                 [ -n \"$H\" ] && set -- \"$@\" -h \"$H\"\n\
                 [ -n \"$D\" ] && set -- \"$@\" -d \"$D\"\n\
                 printf '__RESULT__\\n'\n\
                 PGCONNECT_TIMEOUT=5 psql \"$@\" -c {sql} 2>&1\n\
                 printf '__EXIT__\\n%s\\n' \"$?\"\n"
            )
        }
        DbKind::Mysql => {
            let sql = shell_quote(MYSQL_SQL);
            format!(
                "{PASSWORD_PREAMBLE}\
                 H={host}; P={port}; U={user}\n\
                 printf '[client]\\n' > \"$t\"\n\
                 if [ -n \"$pw\" ]; then printf 'password=\"%s\"\\n' \"$(printf '%s' \"$pw\" | sed 's/[\\\\\"]/\\\\&/g')\" >> \"$t\"; fi\n\
                 c=\"\"; for x in mariadb mysql; do if command -v \"$x\" >/dev/null 2>&1; then c=\"$x\"; break; fi; done\n\
                 [ -n \"$c\" ] || {{ echo __NO_CLIENT__; exit 0; }}\n\
                 set -- --defaults-extra-file=\"$t\" --connect-timeout=5 -P \"$P\" -u \"$U\"\n\
                 [ -n \"$H\" ] && set -- \"$@\" -h \"$H\" --protocol=TCP\n\
                 printf '__RESULT__\\n'\n\
                 \"$c\" \"$@\" -B -N -e {sql} 2>&1\n\
                 printf '__EXIT__\\n%s\\n' \"$?\"\n\
                 printf '__REPLICA__\\n'\n\
                 \"$c\" \"$@\" -e 'SHOW REPLICA STATUS\\G' 2>/dev/null || \"$c\" \"$@\" -e 'SHOW SLAVE STATUS\\G' 2>/dev/null\n\
                 true\n"
            )
        }
    }
}

fn client_failure(kind: DbKind, output: &str) -> String {
    let client = match kind {
        DbKind::Postgres => "psql",
        DbKind::Mysql => "mysql",
    };
    let lower = output.to_lowercase();
    let detail = output
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim();
    if lower.contains("password authentication failed") || lower.contains("access denied") {
        "The database refused the user or password".to_string()
    } else if lower.contains("no password supplied") || lower.contains("using password: no") {
        "The database needs a password; add it to this database's settings".to_string()
    } else if lower.contains("could not connect")
        || lower.contains("can't connect")
        || lower.contains("connection refused")
        || lower.contains("timeout expired")
    {
        format!("{client} could not reach the database: {detail}")
    } else {
        format!("{client} failed: {detail}")
    }
}

fn result_section(
    kind: DbKind,
    output: &str,
) -> Result<(std::collections::HashMap<String, String>, String), String> {
    if output.lines().any(|line| line.trim() == "__NO_CLIENT__") {
        return Err(match kind {
            DbKind::Postgres => "psql is not installed on this server",
            DbKind::Mysql => "Neither mysql nor mariadb is installed on this server",
        }
        .to_string());
    }
    let sections = split_sections(output);
    let result = sections.get("RESULT").cloned().unwrap_or_default();
    if sections.get("EXIT").map(|exit| exit.trim()) != Some("0") {
        return Err(client_failure(kind, &result));
    }
    Ok((sections, result))
}

pub fn parse_postgres(output: &str) -> Result<DbStatus, String> {
    let (_, result) = result_section(DbKind::Postgres, output)?;
    let line = result
        .lines()
        .find(|line| !line.trim().is_empty())
        .ok_or_else(|| {
            "The database answered without statistics; check the database name".to_string()
        })?;
    let fields: Vec<&str> = line.split('\t').collect();
    if fields.len() < 16 {
        return Err(format!("psql returned an unexpected row: {line}"));
    }
    let int = |index: usize| fields[index].trim().parse::<u64>().ok();
    let (hit, read) = (int(9), int(10));
    let is_replica = fields[14].trim() == "t";
    Ok(DbStatus {
        version: Some(fields[0].trim().to_string()).filter(|version| !version.is_empty()),
        connections: int(1),
        max_connections: int(2),
        active: int(3),
        idle_in_transaction: int(4),
        waiting_on_locks: int(5),
        longest_query_secs: int(6),
        commits: int(7),
        rollbacks: int(8),
        cache_hit_percent: hit_ratio(hit, read),
        deadlocks: int(11),
        size_bytes: int(12),
        uptime_secs: int(13),
        is_replica,
        replication_lag_secs: if is_replica { int(15) } else { None },
        ..DbStatus::default()
    })
}

pub fn parse_mysql(output: &str) -> Result<DbStatus, String> {
    let (sections, result) = result_section(DbKind::Mysql, output)?;
    let values: std::collections::HashMap<String, String> = result
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    let int = |name: &str| values.get(name).and_then(|value| value.parse::<u64>().ok());
    let requests = int("innodb_buffer_pool_read_requests");
    let disk_reads = int("innodb_buffer_pool_reads");
    let cache_hit_percent = match (requests, disk_reads) {
        (Some(requests), Some(reads)) if requests > 0 => {
            Some(requests.saturating_sub(reads) as f64 / requests as f64 * 100.0)
        }
        _ => None,
    };
    let replica = sections.get("REPLICA").map(String::as_str).unwrap_or("");
    let lag_line = replica.lines().map(str::trim).find(|line| {
        line.starts_with("Seconds_Behind_Source:") || line.starts_with("Seconds_Behind_Master:")
    });
    Ok(DbStatus {
        version: values.get("version").cloned(),
        connections: int("threads_connected"),
        max_connections: int("max_connections"),
        active: int("threads_running"),
        commits: int("com_commit"),
        rollbacks: int("com_rollback"),
        queries: int("questions"),
        slow_queries: int("slow_queries"),
        cache_hit_percent,
        deadlocks: int("innodb_deadlocks"),
        uptime_secs: int("uptime"),
        is_replica: lag_line.is_some(),
        replication_lag_secs: lag_line
            .and_then(|line| line.split_once(':'))
            .and_then(|(_, value)| value.trim().parse().ok()),
        ..DbStatus::default()
    })
}

fn hit_ratio(hit: Option<u64>, read: Option<u64>) -> Option<f64> {
    match (hit, read) {
        (Some(hit), Some(read)) if hit + read > 0 => Some(hit as f64 / (hit + read) as f64 * 100.0),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance(kind: DbKind, password_user: &str) -> DbInstance {
        DbInstance {
            id: "db1".into(),
            name: "main".into(),
            kind,
            profile_id: "p1".into(),
            host: "127.0.0.1".into(),
            port: kind.default_port(),
            user: password_user.into(),
            database: "app".into(),
            has_password: true,
        }
    }

    #[test]
    fn reads_a_postgres_primary() {
        let output = "__RESULT__\n16.4\t42\t200\t3\t1\t0\t12\t98765\t12\t990000\t10000\t2\t53687091200\t86400\tf\t\n__EXIT__\n0\n";
        let status = parse_postgres(output).unwrap();
        assert_eq!(status.version.as_deref(), Some("16.4"));
        assert_eq!(
            (status.connections, status.max_connections),
            (Some(42), Some(200))
        );
        assert_eq!(
            (
                status.active,
                status.idle_in_transaction,
                status.waiting_on_locks
            ),
            (Some(3), Some(1), Some(0))
        );
        assert_eq!(status.longest_query_secs, Some(12));
        assert_eq!(status.commits, Some(98765));
        assert_eq!(status.cache_hit_percent, Some(99.0));
        assert_eq!(status.size_bytes, Some(53_687_091_200));
        assert!(!status.is_replica);
        assert_eq!(status.replication_lag_secs, None);
    }

    #[test]
    fn reads_postgres_replica_lag() {
        let output =
            "__RESULT__\n16.4\t5\t100\t0\t0\t0\t0\t1\t0\t0\t0\t0\t1\t60\tt\t7\n__EXIT__\n0\n";
        let status = parse_postgres(output).unwrap();
        assert!(status.is_replica);
        assert_eq!(status.replication_lag_secs, Some(7));
        assert_eq!(status.cache_hit_percent, None);
    }

    #[test]
    fn explains_postgres_failures() {
        let refused = "__RESULT__\npsql: error: connection to server at \"127.0.0.1\", port 5432 failed: FATAL:  password authentication failed for user \"app\"\n__EXIT__\n2\n";
        assert_eq!(
            parse_postgres(refused).unwrap_err(),
            "The database refused the user or password"
        );
        assert_eq!(
            parse_postgres("__NO_CLIENT__\n").unwrap_err(),
            "psql is not installed on this server"
        );
        let down = "__RESULT__\npsql: error: connection to server at \"127.0.0.1\", port 5432 failed: Connection refused\n__EXIT__\n2\n";
        assert!(parse_postgres(down)
            .unwrap_err()
            .starts_with("psql could not reach the database"));
    }

    #[test]
    fn reads_mysql_status_and_replica_lag() {
        let output = "__RESULT__\nCom_commit\t500\nCom_rollback\t4\nInnodb_buffer_pool_read_requests\t10000\nInnodb_buffer_pool_reads\t100\nInnodb_deadlocks\t1\nQuestions\t123456\nSlow_queries\t9\nThreads_connected\t17\nThreads_running\t2\nUptime\t3600\nmax_connections\t151\nversion\t10.11.8-MariaDB\n__EXIT__\n0\n__REPLICA__\n*************************** 1. row ***************************\n             Slave_IO_State: Waiting for master\n      Seconds_Behind_Master: 3\n";
        let status = parse_mysql(output).unwrap();
        assert_eq!(status.version.as_deref(), Some("10.11.8-MariaDB"));
        assert_eq!(
            (status.connections, status.max_connections, status.active),
            (Some(17), Some(151), Some(2))
        );
        assert_eq!(
            (status.queries, status.slow_queries),
            (Some(123_456), Some(9))
        );
        assert_eq!(status.cache_hit_percent, Some(99.0));
        assert!(status.is_replica);
        assert_eq!(status.replication_lag_secs, Some(3));
    }

    #[test]
    fn a_mysql_primary_has_no_lag() {
        let output = "__RESULT__\nThreads_connected\t1\n__EXIT__\n0\n__REPLICA__\n";
        let status = parse_mysql(output).unwrap();
        assert!(!status.is_replica);
        let denied = "__RESULT__\nERROR 1045 (28000): Access denied for user 'app'@'localhost' (using password: YES)\n__EXIT__\n1\n__REPLICA__\n";
        assert_eq!(
            parse_mysql(denied).unwrap_err(),
            "The database refused the user or password"
        );
    }

    #[test]
    fn the_password_never_enters_the_script() {
        for kind in [DbKind::Postgres, DbKind::Mysql] {
            let script = status_script(&instance(kind, "app"));
            assert!(script.starts_with("IFS= read -r pw"), "{kind:?}");
            assert!(script.contains("trap 'rm -f \"$t\"' EXIT"));
            assert!(!script.contains("PGPASSWORD"));
            assert!(!script.contains("--password"));
            assert!(!script.contains(" -p\"$pw\""));
        }
    }

    #[test]
    fn fields_are_quoted_for_the_shell() {
        let mut hostile = instance(DbKind::Postgres, "a'; rm -rf ~; '");
        hostile.database = "$(reboot)".into();
        let script = status_script(&hostile);
        assert!(script.contains("U='a'\\''; rm -rf ~; '\\'''"));
        assert!(script.contains("D='$(reboot)'"));
    }

    #[test]
    fn the_status_scripts_are_valid_sh() {
        for kind in [DbKind::Postgres, DbKind::Mysql] {
            let script = status_script(&instance(kind, "app"));
            let status = std::process::Command::new("sh")
                .arg("-n")
                .arg("-c")
                .arg(&script)
                .status()
                .unwrap();
            assert!(status.success(), "{kind:?}:\n{script}");
        }
    }

    /// Runs the real scripts with fake clients that report what they were
    /// given: the password reaches them only through the private file.
    #[cfg(unix)]
    #[test]
    fn the_password_reaches_the_client_only_through_a_removed_private_file() {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt as _;
        let bin = std::env::temp_dir().join(format!("gputerm-db-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&bin).unwrap();
        let fake = "#!/bin/sh\nf=\"$PGPASSFILE\"\nfor a in \"$@\"; do case \"$a\" in --defaults-extra-file=*) f=\"${a#--defaults-extra-file=}\";; esac; done\nprintf 'ARGS %s\\n' \"$*\"\nprintf 'FILE %s\\n' \"$f\"\nprintf 'MODE %s\\n' \"$(ls -l \"$f\" | cut -c1-10)\"\nsed 's/^/CONTENT /' \"$f\"\n";
        for name in ["psql", "mysql"] {
            let path = bin.join(name);
            std::fs::write(&path, fake).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let password = "p:a\"ss\\word";
        for kind in [DbKind::Postgres, DbKind::Mysql] {
            let mut child = std::process::Command::new("sh")
                .arg("-c")
                .arg(status_script(&instance(kind, "app")))
                .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            writeln!(child.stdin.take().unwrap(), "{password}").unwrap();
            let output = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
            let args = output
                .lines()
                .find(|line| line.starts_with("ARGS "))
                .unwrap();
            assert!(!args.contains("word"), "{kind:?} argv: {args}");
            let mode = output
                .lines()
                .find(|line| line.starts_with("MODE "))
                .unwrap();
            assert_eq!(mode, "MODE -rw-------", "{kind:?}");
            let content: Vec<&str> = output
                .lines()
                .filter_map(|line| line.strip_prefix("CONTENT "))
                .collect();
            match kind {
                DbKind::Postgres => assert_eq!(content, vec![r#"*:5432:*:app:p\:a"ss\\word"#]),
                // Status, then replica status: both read the same file.
                DbKind::Mysql => assert_eq!(
                    content,
                    [["[client]", r#"password="p:a\"ss\\word""#]; 2].concat()
                ),
            }
            let file = output
                .lines()
                .find_map(|line| line.strip_prefix("FILE "))
                .unwrap();
            assert!(!std::path::Path::new(file).exists(), "{kind:?} left {file}");
        }
        std::fs::remove_dir_all(&bin).unwrap();
    }

    fn temp_list() -> PathBuf {
        std::env::temp_dir().join(format!("gputerm-db-list-{}.json", Uuid::new_v4()))
    }

    fn input(password: Option<&str>) -> DbInstanceInput {
        DbInstanceInput {
            id: None,
            name: "main".into(),
            kind: DbKind::Postgres,
            profile_id: "p1".into(),
            host: String::new(),
            port: None,
            user: "monitor".into(),
            database: String::new(),
            password: password.map(str::to_string),
        }
    }

    /// A vault that is locked, as `SecureCredentialStore` reports it.
    struct LockedVault;

    impl CredentialStore for LockedVault {
        fn set_password(&self, _: &str, _: String) -> Result<(), String> {
            Err("Credential vault is locked; unlock it with the GpuTerm master password".into())
        }
        fn get_password(&self, _: &str) -> Result<Option<String>, String> {
            Err("Credential vault is locked; unlock it with the GpuTerm master password".into())
        }
        fn clear_password(&self, _: &str) -> Result<(), String> {
            Err("Credential vault is locked; unlock it with the GpuTerm master password".into())
        }
    }

    #[test]
    fn passwords_are_kept_in_the_vault_not_the_list() {
        use crate::ssh::credentials::MemoryCredentialStore;
        let path = temp_list();
        let vault = MemoryCredentialStore::default();
        let saved = save_instance_at(&path, &vault, input(Some("s3cret"))).unwrap();
        let id = saved[0].id.clone();
        assert!(saved[0].has_password);
        assert_eq!(saved[0].port, 5432);
        assert_eq!(
            vault.get_password(&format!("db:{id}")).unwrap().as_deref(),
            Some("s3cret")
        );
        let file = std::fs::read_to_string(&path).unwrap();
        assert!(!file.contains("s3cret"));

        // No password in the form keeps the saved one.
        let kept = save_instance_at(
            &path,
            &vault,
            DbInstanceInput {
                id: Some(id.clone()),
                name: "renamed".into(),
                ..input(None)
            },
        )
        .unwrap();
        assert!(kept[0].has_password);
        assert_eq!(kept[0].name, "renamed");

        // An empty one removes it.
        let removed = save_instance_at(
            &path,
            &vault,
            DbInstanceInput {
                id: Some(id.clone()),
                ..input(Some(""))
            },
        )
        .unwrap();
        assert!(!removed[0].has_password);
        assert_eq!(vault.get_password(&format!("db:{id}")).unwrap(), None);

        save_instance_at(
            &path,
            &vault,
            DbInstanceInput {
                id: Some(id.clone()),
                ..input(Some("again"))
            },
        )
        .unwrap();
        assert!(delete_instance_at(&path, &vault, &id).unwrap().is_empty());
        assert!(vault.is_empty());
        assert!(list_instances_at(&path).unwrap().is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_locked_vault_is_explained() {
        let path = temp_list();
        let error = save_instance_at(&path, &LockedVault, input(Some("s3cret"))).unwrap_err();
        assert_eq!(
            error,
            "Unlock the credential vault to save the database password"
        );
        assert!(!path.exists(), "nothing is saved without its password");
        // Without a password the vault is not needed.
        let saved = save_instance_at(&path, &LockedVault, input(None)).unwrap();
        assert!(!saved[0].has_password);
        assert!(delete_instance_at(&path, &LockedVault, &saved[0].id)
            .unwrap()
            .is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rejects_unusable_input() {
        let valid = DbInstanceInput {
            id: None,
            name: "main".into(),
            kind: DbKind::Mysql,
            profile_id: "p1".into(),
            host: "db.internal".into(),
            port: None,
            user: "app".into(),
            database: String::new(),
            password: None,
        };
        assert!(validate_input(&valid).is_ok());
        for broken in [
            DbInstanceInput {
                name: " ".into(),
                ..valid.clone()
            },
            DbInstanceInput {
                profile_id: String::new(),
                ..valid.clone()
            },
            DbInstanceInput {
                user: String::new(),
                ..valid.clone()
            },
            DbInstanceInput {
                host: "db;reboot".into(),
                ..valid.clone()
            },
            DbInstanceInput {
                port: Some(0),
                ..valid.clone()
            },
            DbInstanceInput {
                database: "a\nb".into(),
                ..valid.clone()
            },
        ] {
            assert!(validate_input(&broken).is_err(), "{broken:?}");
        }
        assert_eq!(DbKind::Postgres.default_port(), 5432);
    }
}
