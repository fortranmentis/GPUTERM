//! Support for the in-app updater (`tauri-plugin-updater`, driven from the
//! frontend): which builds can update themselves, and opening the release
//! page for the ones that cannot.

const RELEASES_URL: &str = "https://github.com/fortranmentis/GPUTERM/releases";

/// "in_app" when this build can install an update itself, "manual" when the
/// user has to download it. On Linux the updater replaces only an AppImage
/// (which sets `APPIMAGE`); a .deb install is owned by the package manager.
#[tauri::command]
pub fn update_install_mode() -> &'static str {
    install_mode(cfg!(target_os = "linux"), std::env::var_os("APPIMAGE").is_some())
}

fn install_mode(linux: bool, appimage: bool) -> &'static str {
    if linux && !appimage {
        "manual"
    } else {
        "in_app"
    }
}

/// The release page for `version` (e.g. "1.3.8-beta"), or the release list
/// when no usable version is given.
fn release_url(version: Option<&str>) -> String {
    match version {
        Some(version)
            if !version.is_empty()
                && version.len() <= 64
                && version
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '+') =>
        {
            format!("{RELEASES_URL}/tag/v{version}")
        }
        _ => RELEASES_URL.to_string(),
    }
}

/// Opens the GitHub release page in the default browser. Only this
/// project's release URLs can be opened, so the frontend cannot use it as a
/// general-purpose launcher.
#[tauri::command]
pub fn open_release_page(version: Option<String>) -> Result<(), String> {
    crate::browser::open_in_browser(&release_url(version.as_deref()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_linux_package_install_is_manual() {
        assert_eq!(install_mode(true, false), "manual");
        assert_eq!(install_mode(true, true), "in_app");
        assert_eq!(install_mode(false, false), "in_app");
    }

    #[test]
    fn release_url_accepts_only_version_strings() {
        assert_eq!(
            release_url(Some("1.3.8-beta")),
            "https://github.com/fortranmentis/GPUTERM/releases/tag/v1.3.8-beta"
        );
        assert_eq!(release_url(None), RELEASES_URL);
        assert_eq!(release_url(Some("")), RELEASES_URL);
        assert_eq!(release_url(Some("1.0/../../evil")), RELEASES_URL);
        assert_eq!(release_url(Some("1.0?x=1")), RELEASES_URL);
    }
}
