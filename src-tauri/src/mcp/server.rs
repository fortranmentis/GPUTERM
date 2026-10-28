//! The loopback listener inside the app. Each relay connection first sends
//! `{"token": …}` (the token from the endpoint file, which only this user
//! can read), then speaks line-delimited JSON-RPC.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rand_core::{OsRng, RngCore};
use serde_json::{json, Value};
use tauri::AppHandle;

use super::protocol::{handle_line, Conversation, ToolHost, ToolOutcome};
use super::{current_settings, endpoint_path, tools, McpState};
use crate::ssh::managed_key::write_private_file;
use tauri::Manager;

const ACCEPT_POLL: Duration = Duration::from_millis(200);
/// The relay sends its token immediately; anything slower is not a relay.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// One JSON-RPC line; tool arguments are small.
const MAX_LINE_BYTES: usize = 1024 * 1024;

static NEXT_CONNECTION: AtomicU64 = AtomicU64::new(1);

/// Open relay connections, so stopping the server can close them too.
type Connections = Arc<Mutex<HashMap<u64, TcpStream>>>;

pub struct ServerHandle {
    pub port: u16,
    stop: Arc<AtomicBool>,
    connections: Connections,
    thread: Option<JoinHandle<()>>,
}

impl ServerHandle {
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        if let Ok(connections) = self.connections.lock() {
            for stream in connections.values() {
                let _ = stream.shutdown(Shutdown::Both);
            }
        }
        let _ = std::fs::remove_file(endpoint_path());
    }
}

pub fn start(app: AppHandle) -> Result<ServerHandle, String> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .map_err(|error| format!("Could not open the MCP port: {error}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("Could not configure the MCP port: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| format!("Could not read the MCP port: {error}"))?
        .port();
    let token = new_token();
    let endpoint = json!({ "port": port, "token": token, "pid": std::process::id() });
    write_private_file(&endpoint_path(), endpoint.to_string().as_bytes())?;

    let stop = Arc::new(AtomicBool::new(false));
    let connections = Connections::default();
    let thread_stop = Arc::clone(&stop);
    let thread_connections = Arc::clone(&connections);
    let thread = thread::Builder::new()
        .name("mcp-server".into())
        .spawn(move || accept_loop(app, listener, token, thread_stop, thread_connections))
        .map_err(|error| format!("Could not start the MCP server: {error}"))?;
    Ok(ServerHandle {
        port,
        stop,
        connections,
        thread: Some(thread),
    })
}

fn new_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Compares in time independent of where the first difference is.
fn tokens_match(expected: &str, given: &str) -> bool {
    expected.len() == given.len()
        && expected
            .bytes()
            .zip(given.bytes())
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}

fn accept_loop(
    app: AppHandle,
    listener: TcpListener,
    token: String,
    stop: Arc<AtomicBool>,
    connections: Connections,
) {
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, peer)) => {
                if !peer.ip().is_loopback() {
                    continue;
                }
                let id = NEXT_CONNECTION.fetch_add(1, Ordering::SeqCst);
                if let (Ok(clone), Ok(mut open)) = (stream.try_clone(), connections.lock()) {
                    open.insert(id, clone);
                }
                let app = app.clone();
                let token = token.clone();
                let stop = Arc::clone(&stop);
                let connections = Arc::clone(&connections);
                let _ = thread::Builder::new()
                    .name("mcp-connection".into())
                    .spawn(move || {
                        let _ = serve_connection(&app, id, stream, &token, &stop);
                        if let Ok(mut open) = connections.lock() {
                            open.remove(&id);
                        }
                    });
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(ACCEPT_POLL);
            }
            Err(_) => thread::sleep(ACCEPT_POLL),
        }
    }
}

/// Reads one line, refusing ones longer than `MAX_LINE_BYTES`. None at EOF.
fn read_line(reader: &mut impl BufRead) -> std::io::Result<Option<String>> {
    let mut buffer = Vec::new();
    let read = std::io::Read::take(&mut *reader, MAX_LINE_BYTES as u64 + 1)
        .read_until(b'\n', &mut buffer)?;
    if read == 0 {
        return Ok(None);
    }
    if buffer.len() > MAX_LINE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "line too long",
        ));
    }
    Ok(Some(
        String::from_utf8_lossy(&buffer).trim_end().to_string(),
    ))
}

fn serve_connection(
    app: &AppHandle,
    connection: u64,
    stream: TcpStream,
    token: &str,
    stop: &AtomicBool,
) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);

    let hello = read_line(&mut reader)?.unwrap_or_default();
    let given = serde_json::from_str::<Value>(&hello)
        .ok()
        .and_then(|hello| {
            hello
                .get("token")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
    if !tokens_match(token, &given) {
        return Ok(());
    }
    writeln!(writer, "{}", json!({ "ok": true }))?;
    reader.get_ref().set_read_timeout(None)?;

    let clients = Arc::clone(&app.state::<McpState>().clients);
    let host = AppHost { app: app.clone() };
    let mut conversation = Conversation::default();
    let result = (|| {
        while !stop.load(Ordering::SeqCst) {
            let Some(line) = read_line(&mut reader)? else {
                break;
            };
            if line.trim().is_empty() {
                continue;
            }
            let reply = handle_line(&mut conversation, &host, &line);
            if let Ok(mut clients) = clients.lock() {
                clients.insert(connection, conversation.client_label().to_string());
            }
            if let Some(reply) = reply {
                writeln!(writer, "{reply}")?;
                writer.flush()?;
            }
        }
        Ok(())
    })();
    if let Ok(mut clients) = clients.lock() {
        clients.remove(&connection);
    }
    result
}

struct AppHost {
    app: AppHandle,
}

impl ToolHost for AppHost {
    fn allow_actions(&self) -> bool {
        current_settings(&self.app).allow_actions
    }

    fn call(&self, client: &str, tool: &str, arguments: &Value) -> ToolOutcome {
        tools::call(&self.app, client, tool, arguments)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_long_random_hex() {
        let first = new_token();
        assert_eq!(first.len(), 64);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(first, new_token());
    }

    #[test]
    fn only_the_exact_token_matches() {
        let token = new_token();
        assert!(tokens_match(&token, &token));
        assert!(!tokens_match(&token, ""));
        assert!(!tokens_match(&token, &token[..63]));
        let mut wrong = token.clone().into_bytes();
        wrong[63] = if wrong[63] == b'0' { b'1' } else { b'0' };
        assert!(!tokens_match(&token, &String::from_utf8(wrong).unwrap()));
    }

    #[test]
    fn overlong_lines_are_refused() {
        let mut long = vec![b'a'; MAX_LINE_BYTES + 10];
        long.push(b'\n');
        assert!(read_line(&mut &long[..]).is_err());
        assert_eq!(
            read_line(&mut &b"{}\r\n"[..]).unwrap().as_deref(),
            Some("{}")
        );
        assert_eq!(read_line(&mut &b""[..]).unwrap(), None);
    }
}
