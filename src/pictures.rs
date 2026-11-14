use anyhow::Result;

pub const KINDS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

pub fn is_picture(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| KINDS.contains(&e.to_ascii_lowercase().as_str()))
}

pub fn dropped(text: &str) -> Option<Vec<String>> {
    let mut paths = Vec::new();
    let mut cur = String::new();
    let mut chars = text.trim().chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(c) = chars.next() {
        match (c, quote) {
            ('\\', None) => cur.extend(chars.next()),
            ('\'' | '"', None) => quote = Some(c),
            (q, Some(open)) if q == open => quote = None,
            (c, None) if c.is_whitespace() => {
                if !cur.is_empty() {
                    paths.push(std::mem::take(&mut cur));
                }
            }
            (c, _) => cur.push(c),
        }
    }
    if !cur.is_empty() {
        paths.push(cur);
    }
    let all = !paths.is_empty()
        && paths
            .iter()
            .all(|p| is_picture(p) && std::path::Path::new(p).is_file());
    all.then_some(paths)
}

#[cfg(not(target_arch = "wasm32"))]
fn dir() -> Option<std::path::PathBuf> {
    let home = std::path::PathBuf::from(std::env::var_os("HOME")?);
    let base = if cfg!(target_os = "macos") {
        home.join("Library").join("Application Support")
    } else {
        home.join(".local").join("share")
    };
    Some(base.join("wryme").join("pictures"))
}

#[cfg(not(target_arch = "wasm32"))]
pub fn from_clipboard() -> Result<String> {
    use anyhow::Context as _;
    let dir = dir().context("no $HOME")?;
    std::fs::create_dir_all(&dir)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let file = dir.join(format!("clip-{stamp}.png"));
    grab(&file)?;
    let size = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        let _ = std::fs::remove_file(&file);
        anyhow::bail!("no image on the clipboard");
    }
    Ok(file.to_string_lossy().into_owned())
}

#[cfg(target_os = "macos")]
fn grab(file: &std::path::Path) -> Result<()> {
    let script = format!(
        "set png to (the clipboard as «class PNGf»)\n\
         set f to open for access POSIX file \"{}\" with write permission\n\
         write png to f\n\
         close access f",
        file.display()
    );
    let out = std::process::Command::new("/usr/bin/osascript")
        .args(["-e", &script])
        .output()?;
    anyhow::ensure!(out.status.success(), "no image on the clipboard");
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn grab(file: &std::path::Path) -> Result<()> {
    let tries: [(&str, &[&str]); 2] = [
        ("wl-paste", &["--no-newline", "--type", "image/png"]),
        ("xclip", &["-selection", "clipboard", "-t", "image/png", "-o"]),
    ];
    for (tool, args) in tries {
        if let Ok(out) = std::process::Command::new(tool).args(args).output()
            && out.status.success()
            && !out.stdout.is_empty()
        {
            std::fs::write(file, out.stdout)?;
            return Ok(());
        }
    }
    anyhow::bail!("no image on the clipboard")
}

#[cfg(not(any(unix, target_arch = "wasm32")))]
fn grab(_: &std::path::Path) -> Result<()> {
    anyhow::bail!("clipboard images are not supported here yet")
}

#[cfg(target_arch = "wasm32")]
pub fn from_clipboard() -> Result<String> {
    anyhow::bail!("the browser can't attach images")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropped_paths_unescape_and_need_real_pictures() {
        let dir = std::env::temp_dir().join(format!("wryme-drop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("my shot.png");
        let b = dir.join("b.jpg");
        std::fs::write(&a, b"x").unwrap();
        std::fs::write(&b, b"x").unwrap();
        let escaped = format!("{} {}", a.display().to_string().replace(' ', "\\ "), b.display());
        assert_eq!(
            dropped(&escaped),
            Some(vec![a.display().to_string(), b.display().to_string()])
        );
        assert_eq!(dropped(&format!("'{}'", a.display())), Some(vec![a.display().to_string()]));
        assert_eq!(dropped("hello world"), None);
        assert_eq!(dropped(&format!("{} and text", b.display())), None);
    }
}
