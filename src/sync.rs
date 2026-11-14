use anyhow::Result;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Way {
    Link,
    Pick,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Sync {
    pub place: String,
    pub pick: u64,
    #[serde(skip)]
    pub way: Option<Way>,
}

impl Sync {
    pub fn now() -> Self {
        Self {
            place: place(),
            pick: 0,
            way: way(),
        }
    }

    pub fn refresh(&mut self) {
        self.place = place();
        self.way = way();
    }
}

pub fn way() -> Option<Way> {
    if cfg!(any(target_os = "ios", target_os = "android", target_arch = "wasm32")) {
        return Some(Way::Pick);
    }
    imp::linked().map(|_| Way::Link)
}

pub fn place() -> String {
    if let Some(dir) = std::env::var_os("WRYME_CONFIG_DIR") {
        let dir = std::path::PathBuf::from(dir);
        if dir.to_string_lossy().contains("com~apple~CloudDocs") {
            return "iCloud Drive".into();
        }
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        return format!("folder {name}");
    }
    match imp::linked() {
        Some(true) => "iCloud Drive".into(),
        _ if cfg!(target_arch = "wasm32") && crate::config::can_write() => "folder".into(),
        _ => "off".into(),
    }
}

pub fn link(on: bool) -> Result<()> {
    imp::set(on)?;
    publish();
    Ok(())
}

pub const AWAY: bool = cfg!(any(target_os = "ios", target_os = "android"));

pub fn publish() {
    if imp::linked() != Some(true) {
        return;
    }
    let Some(name) = imp::local_name() else {
        return;
    };
    let mine = crate::config::host_pref();
    let theirs = mine.as_deref().is_some_and(|h| !h.ends_with(".local"));
    if mine.as_deref() != Some(name.as_str()) && !theirs {
        let _ = crate::config::write_host(&name);
    }
}

fn split_host(authority: &str) -> (&str, &str) {
    if authority.starts_with('[') {
        let end = authority.find(']').map_or(authority.len(), |i| i + 1);
        return authority.split_at(end);
    }
    authority.split_at(authority.find(':').unwrap_or(authority.len()))
}

pub fn reach(url: &str, host: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let (name, port) = split_host(authority);
    let local = matches!(
        name.to_ascii_lowercase().as_str(),
        "localhost" | "127.0.0.1" | "0.0.0.0" | "[::1]"
    );
    local.then(|| format!("{scheme}://{host}{port}{path}"))
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
mod imp {
    use anyhow::{Context, Result};
    use std::path::{Path, PathBuf};

    const SHARED: [&str; 3] = ["shops.toml", "stations.toml", "prefs.json"];

    fn home() -> Option<PathBuf> {
        std::env::var_os("HOME").map(PathBuf::from)
    }

    fn here() -> Option<PathBuf> {
        home().map(|h| h.join(".config").join("wryme"))
    }

    fn drive() -> Option<PathBuf> {
        if !cfg!(target_os = "macos") || std::env::var_os("WRYME_CONFIG_DIR").is_some() {
            return None;
        }
        let docs = home()?
            .join("Library")
            .join("Mobile Documents")
            .join("com~apple~CloudDocs");
        docs.is_dir().then(|| docs.join("wryme"))
    }

    pub fn linked() -> Option<bool> {
        Some(shared(&here()?, &drive()?))
    }

    pub fn local_name() -> Option<String> {
        if !cfg!(target_os = "macos") {
            return None;
        }
        let out = std::process::Command::new("/usr/sbin/scutil")
            .args(["--get", "LocalHostName"])
            .output()
            .ok()?;
        let name = String::from_utf8(out.stdout).ok()?.trim().to_string();
        (out.status.success() && !name.is_empty()).then(|| format!("{name}.local"))
    }

    pub fn set(on: bool) -> Result<()> {
        let here = here().context("no $HOME")?;
        let drive = drive().context("iCloud Drive is not available")?;
        if on {
            share(&here, &drive, crate::platform::unix_secs() as u64)
        } else {
            unshare(&here, &drive)
        }
    }

    fn shared(here: &Path, drive: &Path) -> bool {
        SHARED.iter().all(|name| {
            std::fs::read_link(here.join(name)).is_ok_and(|target| target == drive.join(name))
        })
    }

    fn share(here: &Path, drive: &Path, stamp: u64) -> Result<()> {
        std::fs::create_dir_all(drive).with_context(|| format!("creating {}", drive.display()))?;
        std::fs::create_dir_all(here).with_context(|| format!("creating {}", here.display()))?;
        for name in SHARED {
            let local = here.join(name);
            let remote = drive.join(name);
            if std::fs::read_link(&local).is_ok() {
                std::fs::remove_file(&local)?;
            } else if local.exists() {
                let keep = if remote.exists() {
                    here.join(format!("{name}.bak-{stamp}"))
                } else {
                    remote.clone()
                };
                std::fs::rename(&local, &keep)
                    .or_else(|_| std::fs::copy(&local, &keep).and_then(|_| std::fs::remove_file(&local)))
                    .with_context(|| format!("moving {}", local.display()))?;
            }
            std::os::unix::fs::symlink(&remote, &local)
                .with_context(|| format!("linking {}", local.display()))?;
        }
        Ok(())
    }

    fn unshare(here: &Path, drive: &Path) -> Result<()> {
        for name in SHARED {
            let local = here.join(name);
            if std::fs::read_link(&local).is_err() {
                continue;
            }
            std::fs::remove_file(&local)?;
            let remote = drive.join(name);
            if remote.exists() {
                std::fs::copy(&remote, &local).with_context(|| format!("copying {}", remote.display()))?;
            }
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn dirs(tag: &str) -> (PathBuf, PathBuf) {
            let root = std::env::temp_dir().join(format!("wryme-cloud-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            (root.join("here"), root.join("drive"))
        }

        fn seed(dir: &Path, text: &str) {
            std::fs::create_dir_all(dir).unwrap();
            for name in SHARED {
                std::fs::write(dir.join(name), format!("{text} {name}")).unwrap();
            }
        }

        #[test]
        fn sharing_moves_files_and_leaves_links() {
            let (here, drive) = dirs("move");
            seed(&here, "mine");
            share(&here, &drive, 1).unwrap();
            assert!(shared(&here, &drive));
            let read = std::fs::read_to_string(here.join("stations.toml")).unwrap();
            assert_eq!(read, "mine stations.toml");
            std::fs::write(here.join("prefs.json"), "edited").unwrap();
            assert_eq!(std::fs::read_to_string(drive.join("prefs.json")).unwrap(), "edited");
        }

        #[test]
        fn sharing_keeps_cloud_copies_and_backs_up_local() {
            let (here, drive) = dirs("keep");
            seed(&here, "mine");
            seed(&drive, "cloud");
            share(&here, &drive, 7).unwrap();
            assert_eq!(std::fs::read_to_string(here.join("shops.toml")).unwrap(), "cloud shops.toml");
            assert_eq!(
                std::fs::read_to_string(here.join("shops.toml.bak-7")).unwrap(),
                "mine shops.toml"
            );
        }

        #[test]
        fn unsharing_copies_back_and_leaves_the_cloud_alone() {
            let (here, drive) = dirs("back");
            seed(&here, "mine");
            share(&here, &drive, 1).unwrap();
            unshare(&here, &drive).unwrap();
            assert!(!shared(&here, &drive));
            assert!(std::fs::read_link(here.join("shops.toml")).is_err());
            assert_eq!(std::fs::read_to_string(here.join("shops.toml")).unwrap(), "mine shops.toml");
            assert!(drive.join("shops.toml").exists());
        }
    }
}

#[cfg(not(all(unix, not(target_arch = "wasm32"))))]
mod imp {
    use anyhow::Result;

    pub fn linked() -> Option<bool> {
        None
    }

    pub fn local_name() -> Option<String> {
        None
    }

    pub fn set(_: bool) -> Result<()> {
        anyhow::bail!("iCloud Drive is not available")
    }
}

#[cfg(test)]
mod tests {
    use super::reach;

    #[test]
    fn local_shops_reach_the_mac() {
        let mac = "Adils-MacBook-Pro.local";
        assert_eq!(
            reach("http://localhost:11434/v1", mac).as_deref(),
            Some("http://Adils-MacBook-Pro.local:11434/v1")
        );
        assert_eq!(
            reach("http://127.0.0.1:8899/v1", mac).as_deref(),
            Some("http://Adils-MacBook-Pro.local:8899/v1")
        );
        assert_eq!(
            reach("http://[::1]:14620", mac).as_deref(),
            Some("http://Adils-MacBook-Pro.local:14620")
        );
        assert_eq!(reach("http://LOCALHOST/v1", mac).as_deref(), Some("http://Adils-MacBook-Pro.local/v1"));
    }

    #[test]
    fn other_shops_stay_put() {
        assert_eq!(reach("https://api.openai.com/v1", "mac.local"), None);
        assert_eq!(reach("http://localhost.example.com/v1", "mac.local"), None);
        assert_eq!(reach("", "mac.local"), None);
    }
}
