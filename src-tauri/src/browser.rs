//! Opens a URL in the user's default browser. Callers decide which URLs are
//! allowed; this only launches the platform handler.

use std::process::Command;

pub(crate) fn open_in_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(url);
        command
    };
    #[cfg(windows)]
    let mut command = {
        let mut command = Command::new("rundll32");
        command.args(["url.dll,FileProtocolHandler", url]);
        command
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Could not open {url}: {error}"))
}
