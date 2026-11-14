use std::ffi::{CStr, CString, c_char};
use std::sync::{Arc, OnceLock};

use tokio::sync::mpsc;
use wryme::control::Intent;
use wryme::serve::Emit;

type Callback = extern "C" fn(*const c_char);

static INBOX: OnceLock<mpsc::UnboundedSender<Intent>> = OnceLock::new();

#[unsafe(no_mangle)]
pub extern "C" fn wryme_start(callback: Callback) {
    let (tx, rx) = mpsc::unbounded_channel();
    if INBOX.set(tx).is_err() {
        return;
    }
    let emit: Emit = Arc::new(move |line: &str| {
        if let Ok(line) = CString::new(line) {
            callback(line.as_ptr());
        }
    });
    std::thread::spawn(move || {
        let _sentry = wryme::instrument();
        let ended = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(anyhow::Error::from)
            .and_then(|rt| rt.block_on(wryme::serve::serve(emit.clone(), rx)));
        match ended {
            Ok(()) => std::process::exit(0),
            Err(e) => wryme::serve::fatal(&emit, &e),
        }
    });
}

#[allow(clippy::missing_safety_doc)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wryme_send(msg: *const c_char) {
    if msg.is_null() {
        return;
    }
    let text = unsafe { CStr::from_ptr(msg) }.to_string_lossy();
    if let (Some(intent), Some(tx)) = (wryme::serve::parse(&text), INBOX.get()) {
        let _ = tx.send(intent);
    }
}

#[cfg(test)]
mod tests {
    fn swift() -> String {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/app");
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "swift"))
            .map(|e| std::fs::read_to_string(e.path()).unwrap())
            .collect()
    }

    fn names<T: serde::Serialize>(all: &[T]) -> Vec<String> {
        all.iter()
            .map(|v| serde_json::to_value(v).unwrap().as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn swift_handles_every_shared_case() {
        use wryme::action::{Edit, Pane, Scope};
        use wryme::app::ViewMode;
        use wryme::config::{NametagMode, TYPEFACES};
        let src = swift();
        let mut wanted = names(Edit::ALL);
        wanted.extend(names(Pane::ALL));
        wanted.extend(names(Scope::ALL));
        wanted.extend(names(&[ViewMode::Page, ViewMode::Scroll]));
        wanted.extend(names(&[NametagMode::Standout, NametagMode::Inlined]));
        wanted.extend(TYPEFACES.iter().map(|(name, _)| name.to_string()));
        let missing: Vec<&String> = wanted
            .iter()
            .filter(|name| !src.contains(&format!("case {name}")) && !src.contains(&format!(", {name}")))
            .collect();
        assert!(missing.is_empty(), "Swift has no case for {missing:?}");
    }
}
