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
    imp::set(on)
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

    pub fn set(_: bool) -> Result<()> {
        anyhow::bail!("iCloud Drive is not available")
    }
}
