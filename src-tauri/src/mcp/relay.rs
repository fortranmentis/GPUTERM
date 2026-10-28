//! `gputerm --mcp-stdio`: what MCP clients launch. It forwards stdio lines to
//! the running app's MCP port. While the app is closed (or MCP is off) it
//! still answers the handshake and tool list itself, and every tool call
//! explains how to turn GpuTerm on; it reconnects on the next message.

use std::io::{self, BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpStream};
use std::time::Duration;

use serde_json::{json, Value};

use super::endpoint_path;
use super::protocol::{handle_line, Conversation, ToolHost, ToolOutcome};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

const UNAVAILABLE: &str = "GpuTerm is not running, or its MCP server is turned off. Open GpuTerm, click MCP at the bottom of the sidebar and turn on \"Allow MCP clients\", then try again.";

struct Offline;

impl ToolHost for Offline {
    // Listing the actions too means a call reaches `call` and gets the
    // explanation below instead of an "unknown tool" protocol error.
    fn allow_actions(&self) -> bool {
        true
    }

    fn call(&self, _client: &str, _tool: &str, _arguments: &Value) -> ToolOutcome {
        Err(UNAVAILABLE.to_string())
    }
}

struct Upstream {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl Upstream {
    fn connect() -> Option<Self> {
        let endpoint: Value = serde_json::from_slice(&std::fs::read(endpoint_path()).ok()?).ok()?;
        let port = u16::try_from(endpoint.get("port")?.as_u64()?).ok()?;
        let token = endpoint.get("token")?.as_str()?;
        let stream =
            TcpStream::connect_timeout(&(Ipv4Addr::LOCALHOST, port).into(), CONNECT_TIMEOUT)
                .ok()?;
        stream.set_read_timeout(Some(CONNECT_TIMEOUT)).ok()?;
        let mut upstream = Upstream {
            writer: stream.try_clone().ok()?,
            reader: BufReader::new(stream),
        };
        writeln!(upstream.writer, "{}", json!({ "token": token })).ok()?;
        let hello = upstream.read_line().ok()??;
        if serde_json::from_str::<Value>(&hello).ok()?.get("ok") != Some(&Value::Bool(true)) {
            return None;
        }
        // Tool calls can wait minutes for the user's approval.
        upstream.reader.get_ref().set_read_timeout(None).ok()?;
        Some(upstream)
    }

    fn read_line(&mut self) -> io::Result<Option<String>> {
        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        Ok(Some(line.trim_end().to_string()))
    }

    /// Sends one line; returns the reply if `line` expects one.
    fn exchange(&mut self, line: &str) -> io::Result<Option<String>> {
        writeln!(self.writer, "{line}")?;
        self.writer.flush()?;
        if !expects_reply(line) {
            return Ok(None);
        }
        self.read_line()?.map(Some).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "GpuTerm closed the connection",
            )
        })
    }
}

/// Whether the server answers `line`, mirroring `protocol::handle_line`:
/// requests and malformed input get a reply, notifications do not.
fn expects_reply(line: &str) -> bool {
    let Ok(message) = serde_json::from_str::<Value>(line) else {
        return true;
    };
    let Some(object) = message.as_object() else {
        return true;
    };
    match object.get("method") {
        Some(_) => object.contains_key("id"),
        None => {
            object.contains_key("id")
                && !object.contains_key("result")
                && !object.contains_key("error")
        }
    }
}

fn is_initialize(line: &str) -> bool {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|message| {
            message
                .get("method")
                .and_then(Value::as_str)
                .map(|m| m == "initialize")
        })
        .unwrap_or(false)
}

/// Runs until stdin closes. Returns the process exit code.
pub fn run() -> i32 {
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    let mut conversation = Conversation::default();
    // Replayed on a fresh connection so the app learns the client's name.
    let mut initialize: Option<String> = None;
    let mut upstream: Option<Upstream> = None;

    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            break;
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let first_initialize = is_initialize(line);
        if first_initialize {
            initialize = Some(line.to_string());
        }
        if upstream.is_none() {
            upstream = Upstream::connect().and_then(|mut fresh| match &initialize {
                Some(init) if !first_initialize => fresh.exchange(init).ok().map(|_| fresh),
                _ => Some(fresh),
            });
        }
        let reply = match upstream.as_mut().map(|up| up.exchange(line)) {
            Some(Ok(reply)) => reply,
            Some(Err(_)) | None => {
                upstream = None;
                handle_line(&mut conversation, &Offline, line)
            }
        };
        if let Some(reply) = reply {
            if writeln!(stdout, "{reply}")
                .and_then(|()| stdout.flush())
                .is_err()
            {
                break;
            }
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_are_expected_for_requests_and_bad_input_only() {
        assert!(expects_reply(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#
        ));
        assert!(!expects_reply(
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#
        ));
        assert!(!expects_reply(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#));
        assert!(expects_reply("{broken"));
        assert!(expects_reply("[]"));
    }

    #[test]
    fn offline_calls_explain_how_to_turn_gputerm_on() {
        let mut conversation = Conversation::default();
        let call = json!({
            "jsonrpc": "2.0", "id": 7, "method": "tools/call",
            "params": { "name": "run_command", "arguments": { "host": "a", "command": "id" } }
        });
        let reply: Value = serde_json::from_str(
            &handle_line(&mut conversation, &Offline, &call.to_string()).unwrap(),
        )
        .unwrap();
        assert_eq!(reply["result"]["isError"], true);
        assert_eq!(reply["result"]["content"][0]["text"], UNAVAILABLE);
    }
}
