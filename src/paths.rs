//! Where termwall's files live.
//!
//! The app folder holds the page (index.html, logos.js, fonts). A clone or the Windows
//! installer keeps everything there, settings included, as the Python version always did. A
//! system package (a read-only /usr/share/termwall marked by a SYSTEM-INSTALL file) keeps the
//! settings in $XDG_CONFIG_HOME/termwall, and token.js and share.json in $XDG_DATA_HOME/termwall,
//! which is also where the wallpaper loads the page from (links to the packaged files, so
//! token.js sits next to index.html).

use std::env;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const PAGE_ASSETS: [&str; 3] = ["index.html", "logos.js", "fonts"];

#[derive(Clone, Debug)]
pub struct Paths {
    pub app_dir: PathBuf,
    pub system_install: bool,
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub style_file: PathBuf,
    pub theme_file: PathBuf,
    pub token_file: PathBuf,
    pub share_file: PathBuf,
}

impl Paths {
    fn detect() -> Paths {
        let app_dir = app_dir().to_path_buf();
        let system_install = app_dir.join("SYSTEM-INSTALL").exists();
        let xdg = |var: &str, fallback: &str| -> PathBuf {
            let base = env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home_dir().join(fallback));
            base.join("termwall")
        };
        let (config_dir, data_dir) = if system_install {
            (xdg("XDG_CONFIG_HOME", ".config"), xdg("XDG_DATA_HOME", ".local/share"))
        } else {
            (app_dir.clone(), app_dir.clone())
        };
        let from_env = |var: &str| env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from);
        Paths {
            style_file: from_env("TERMWALL_STYLE").unwrap_or_else(|| config_dir.join("termwall.toml")),
            theme_file: from_env("TERMWALL_THEME").unwrap_or_else(|| config_dir.join("theme.json")),
            token_file: data_dir.join("token.js"),
            share_file: data_dir.join("share.json"),
            app_dir,
            system_install,
            config_dir,
            data_dir,
        }
    }

    /// Everything a test writes goes into `dir`; the page and logos.js stay the app's own.
    pub fn in_dir(dir: &Path) -> Paths {
        Paths {
            app_dir: app_dir().to_path_buf(),
            system_install: false,
            config_dir: dir.to_path_buf(),
            data_dir: dir.to_path_buf(),
            style_file: dir.join("termwall.toml"),
            theme_file: dir.join("theme.json"),
            token_file: dir.join("token.js"),
            share_file: dir.join("share.json"),
        }
    }

    pub fn page_file(&self) -> PathBuf {
        self.app_dir.join("index.html")
    }

    /// logo.txt and logo images sit next to termwall.toml.
    pub fn style_dir(&self) -> PathBuf {
        self.style_file.parent().map(Path::to_path_buf).unwrap_or_default()
    }

    pub fn logo_file(&self) -> PathBuf {
        self.style_dir().join("logo.txt")
    }
}

pub fn get() -> &'static Paths {
    static P: OnceLock<Paths> = OnceLock::new();
    P.get_or_init(Paths::detect)
}

/// The folder with index.html: $TERMWALL_HOME, the binary's folder or one of its parents (so
/// target/release/termwall in a checkout finds the checkout), or the packaged share folder.
pub fn app_dir() -> &'static Path {
    static D: OnceLock<PathBuf> = OnceLock::new();
    D.get_or_init(|| {
        if let Some(h) = env::var_os("TERMWALL_HOME").filter(|v| !v.is_empty()) {
            return PathBuf::from(h);
        }
        if cfg!(test) {
            return PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        }
        let exe_dir = env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_default();
        for dir in exe_dir.ancestors().take(5) {
            if dir.join("index.html").is_file() {
                return dir.to_path_buf();
            }
        }
        #[cfg(unix)]
        for dir in ["/usr/share/termwall", "/usr/local/share/termwall"] {
            if Path::new(dir).join("index.html").is_file() {
                return PathBuf::from(dir);
            }
        }
        exe_dir
    })
}

pub fn logos_js() -> PathBuf {
    app_dir().join("logos.js")
}

pub fn home_dir() -> PathBuf {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    env::var_os(var).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}
