//! Positive game hints for fullscreen navigation. Never infer games from
//! XWayland, fullscreen, window titles, pointer grabs, or a launcher process.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use smithay::desktop::Window;
use smithay::reexports::wayland_protocols::wp::content_type::v1::server::wp_content_type_v1::Type;
use smithay::reexports::wayland_server::{DisplayHandle, Resource};
use smithay::wayland::compositor::with_states;
use smithay::wayland::content_type::ContentTypeSurfaceCachedState;
use smithay::wayland::seat::WaylandFocus;

#[derive(Debug, Default)]
pub(crate) struct GameApplications {
    desktop: Arc<OnceLock<HashMap<String, bool>>>,
}

impl GameApplications {
    pub fn load() -> Self {
        let games = Self::default();
        let desktop = games.desktop.clone();
        // Directory and desktop-file I/O stays off the compositor event loop.
        // A missing hint defaults to ordinary window behavior; rules override it.
        let _ = std::thread::Builder::new()
            .name("halley-game-metadata".into())
            .spawn(move || {
                let _ = desktop.set(desktop_applications(&data_roots()));
            });
        games
    }

    pub fn is_game(&self, window: &Window, display: &DisplayHandle, rule: Option<bool>) -> bool {
        if let Some(game) = rule {
            return game;
        }
        if let Some(surface) = window.wl_surface()
            && window.toplevel().is_some()
            && with_states(surface.as_ref(), |states| {
                *states
                    .cached_state
                    .get::<ContentTypeSurfaceCachedState>()
                    .current()
                    .content_type()
                    == Type::Game
            })
        {
            return true;
        }
        let identity = super::rules::identity(window);
        if identity.app_id.as_deref().is_some_and(|app_id| {
            steam_window_id(app_id)
                || self
                    .desktop
                    .get()
                    .and_then(|entries| entries.get(&app_id.to_lowercase()))
                    .copied()
                    .unwrap_or(false)
        }) {
            return true;
        }
        // X11's Wayland peer is XWayland itself, so use the window's PID.
        // Never walk launcher ancestry: that would classify unrelated UIs.
        #[cfg(feature = "xwayland")]
        let x11_pid = window.x11_surface().and_then(|surface| surface.pid());
        #[cfg(not(feature = "xwayland"))]
        let x11_pid: Option<u32> = None;
        let pid = if window.toplevel().is_some() {
            window
                .wl_surface()
                .and_then(|surface| surface.client())
                .and_then(|client| client.get_credentials(display).ok())
                .and_then(|credentials| u32::try_from(credentials.pid).ok())
        } else {
            x11_pid
        };
        pid.is_some_and(process_has_game_id)
    }
}

fn positive_id(value: &[u8]) -> bool {
    !value.is_empty() && value.iter().all(u8::is_ascii_digit) && value.iter().any(|b| *b != b'0')
}

fn steam_window_id(app_id: &str) -> bool {
    app_id
        .strip_prefix("steam_app_")
        .is_some_and(|id| positive_id(id.as_bytes()))
}

fn game_environment(bytes: &[u8]) -> bool {
    bytes.split(|b| *b == 0).any(|entry| {
        [b"SteamAppId=".as_slice(), b"SteamGameId=".as_slice()]
            .into_iter()
            .any(|key| entry.strip_prefix(key).is_some_and(positive_id))
    })
}

fn process_has_game_id(pid: u32) -> bool {
    let Ok(file) = std::fs::File::open(format!("/proc/{pid}/environ")) else {
        return false;
    };
    // Bounded, local procfs access only for fullscreen navigation candidates.
    // Do not retain or log any process environment contents.
    let mut bytes = Vec::new();
    file.take(128 * 1024).read_to_end(&mut bytes).is_ok() && game_environment(&bytes)
}

fn data_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(path) = std::env::var_os("XDG_DATA_HOME").filter(|path| !path.is_empty()) {
        roots.push(PathBuf::from(path));
    } else if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home).join(".local/share"));
    }
    roots.extend(std::env::split_paths(
        &std::env::var_os("XDG_DATA_DIRS")
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".into()),
    ));
    roots
        .into_iter()
        .filter(|path| path.is_absolute())
        .collect()
}

fn desktop_applications(roots: &[PathBuf]) -> HashMap<String, bool> {
    let mut entries = HashMap::new();
    let mut seen = HashSet::new();
    for root in roots {
        let applications = root.join("applications");
        visit_desktop_files(&applications, 0, &mut |path| {
            let Some(id) = path
                .strip_prefix(&applications)
                .ok()
                .and_then(|relative| relative.to_str().map(|value| value.replace('/', "-")))
            else {
                return;
            };
            // XDG precedence includes user overrides hiding a system entry.
            if !seen.insert(id.clone()) {
                return;
            }
            let Ok(text) = std::fs::read_to_string(path) else {
                return;
            };
            let (game, class) = desktop_game(&text);
            if let Some(stem) = id.strip_suffix(".desktop") {
                entries.entry(stem.to_lowercase()).or_insert(game);
            }
            if let Some(class) = class {
                entries.entry(class.to_lowercase()).or_insert(game);
            }
        });
    }
    entries
}

fn visit_desktop_files(root: &Path, depth: usize, visit: &mut impl FnMut(&Path)) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut paths = entries
        .flatten()
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        if path.is_dir() && depth < 4 {
            visit_desktop_files(&path, depth + 1, visit);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "desktop")
        {
            visit(&path);
        }
    }
}

fn desktop_game(text: &str) -> (bool, Option<&str>) {
    let mut in_entry = false;
    let mut application = false;
    let mut hidden = false;
    let mut game = false;
    let mut other_main_category = false;
    let mut class = None;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
        } else if in_entry && let Some((key, value)) = line.split_once('=') {
            let value = value.trim();
            match key.trim() {
                "Type" => application = value == "Application",
                "Hidden" => hidden = value == "true",
                "Categories" => {
                    game = value.split(';').any(|category| category == "Game");
                    // Launchers and development tools commonly include Game
                    // alongside their own main category. That is ambiguous,
                    // not positive evidence that this window is a game.
                    other_main_category = value.split(';').any(|category| {
                        matches!(
                            category,
                            "AudioVideo"
                                | "Audio"
                                | "Video"
                                | "Development"
                                | "Education"
                                | "Graphics"
                                | "Network"
                                | "Office"
                                | "Science"
                                | "Settings"
                                | "System"
                                | "Utility"
                        )
                    });
                }
                "StartupWMClass" if !value.is_empty() => class = Some(value),
                _ => {}
            }
        }
    }
    (
        application && !hidden && game && !other_main_category,
        class,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steam_hints_require_positive_numeric_ids() {
        assert!(steam_window_id("steam_app_440"));
        assert!(game_environment(b"OTHER=x\0SteamAppId=440\0"));
        assert!(game_environment(b"SteamAppId=0\0SteamGameId=12345\0"));
        for app in [
            "steam",
            "steamwebhelper",
            "firefox",
            "steam_app_",
            "steam_app_0",
            "steam_app_12x",
        ] {
            assert!(!steam_window_id(app));
        }
        for env in [
            b"SteamAppId=0\0".as_slice(),
            b"SteamAppId=\0",
            b"SteamAppId=abc\0",
            b"NOT_SteamAppId=440\0",
        ] {
            assert!(!game_environment(env));
        }
    }

    #[test]
    fn desktop_game_category_is_exact_and_main_entry_only() {
        assert_eq!(
            desktop_game(
                "[Desktop Entry]\nType=Application\nCategories=ActionGame;Game;\nStartupWMClass=my-game\n"
            ),
            (true, Some("my-game"))
        );
        for text in [
            "[Desktop Entry]\nType=Application\nCategories=ActionGame;\n",
            "[Desktop Entry]\nType=Application\nCategories=Game;\nHidden=true\n",
            "[Desktop Entry]\nType=Application\nCategories=Network;Game;\n",
            "[Desktop Entry]\nType=Application\nCategories=Utility;Game;\n",
            "[Desktop Entry]\nType=Application\nCategories=WebBrowser;\n[Desktop Action Game]\nCategories=Game;\n",
        ] {
            assert!(!desktop_game(text).0);
        }
    }

    #[test]
    fn desktop_index_honors_user_overrides_and_nested_ids() {
        let root =
            std::env::temp_dir().join(format!("halley-game-metadata-{}", std::process::id()));
        let user = root.join("user");
        let system = root.join("system");
        std::fs::create_dir_all(user.join("applications")).unwrap();
        std::fs::create_dir_all(system.join("applications/vendor")).unwrap();
        let game =
            "[Desktop Entry]\nType=Application\nCategories=Game;\nStartupWMClass=game-class\n";
        std::fs::write(system.join("applications/test.desktop"), game).unwrap();
        std::fs::write(
            user.join("applications/test.desktop"),
            "[Desktop Entry]\nHidden=true\n",
        )
        .unwrap();
        std::fs::write(system.join("applications/vendor/other.desktop"), game).unwrap();
        let entries = desktop_applications(&[user, system]);
        assert_eq!(entries.get("test"), Some(&false));
        assert_eq!(entries.get("vendor-other"), Some(&true));
        assert_eq!(entries.get("game-class"), Some(&true));
        std::fs::remove_dir_all(root).unwrap();
    }
}
