use std::ffi::{CStr, CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::mpsc;
use wryme::control::Intent;
use wryme::serve::Emit;

type Callback = extern "C" fn(*const c_char);

static INBOX: Mutex<Option<mpsc::UnboundedSender<Intent>>> = Mutex::new(None);

fn inbox() -> MutexGuard<'static, Option<mpsc::UnboundedSender<Intent>>> {
    INBOX.lock().unwrap_or_else(PoisonError::into_inner)
}

#[unsafe(no_mangle)]
pub extern "C" fn wryme_start(callback: Callback) {
    let (tx, rx) = mpsc::unbounded_channel();
    {
        let mut inbox = inbox();
        if inbox.as_ref().is_some_and(|t| !t.is_closed()) {
            return;
        }
        *inbox = Some(tx);
    }
    let emit: Emit = Arc::new(move |line: &str| {
        if let Ok(line) = CString::new(line) {
            callback(line.as_ptr());
        }
    });
    std::thread::spawn(move || {
        let _sentry = wryme::instrument();
        let ended = catch_unwind(AssertUnwindSafe(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(anyhow::Error::from)
                .and_then(|rt| rt.block_on(wryme::serve::serve(emit.clone(), rx)))
        }));
        match ended {
            Ok(Ok(())) => std::process::exit(0),
            Ok(Err(e)) => wryme::serve::fatal(&emit, &e),
            Err(panic) => {
                let said = panic
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                    .unwrap_or("unknown");
                wryme::serve::fatal(&emit, &anyhow::anyhow!("wryme stopped: {said}"));
            }
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
    if let (Some(intent), Some(tx)) = (wryme::serve::parse(&text), inbox().as_ref()) {
        let _ = tx.send(intent);
    }
}

#[allow(clippy::missing_safety_doc)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wryme_set_folder(path: *const c_char) {
    if path.is_null() {
        return;
    }
    let path = unsafe { CStr::from_ptr(path) }.to_string_lossy().into_owned();
    wryme::config::share_dir(path.into());
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    fn swift() -> String {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/app");
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "swift"))
            .map(|e| std::fs::read_to_string(e.path()).unwrap())
            .collect()
    }

    fn names<T: serde::Serialize>(all: &[T]) -> BTreeSet<String> {
        all.iter()
            .map(|v| serde_json::to_value(v).unwrap().as_str().unwrap().to_string())
            .collect()
    }

    fn cases(src: &str, name: &str) -> BTreeSet<String> {
        let at = src
            .find(&format!("enum {name}: String"))
            .unwrap_or_else(|| panic!("Swift has no enum {name}"));
        let body = &src[at..];
        let body = &body[body.find('{').unwrap() + 1..body.find('}').unwrap()];
        body.split(|c: char| c == ',' || c.is_whitespace())
            .filter(|t| !t.is_empty() && *t != "case")
            .map(String::from)
            .collect()
    }

    #[test]
    fn swift_enums_match_the_core_both_ways() {
        use wryme::action::{Core, Edit, Pane, Scope};
        use wryme::app::ViewMode;
        use wryme::config::{NAMETAG_MODES, TYPEFACES};
        let src = swift();
        let pairs = [
            ("Command", names(Core::ALL)),
            ("Edit", names(Edit::ALL)),
            ("Pane", names(Pane::ALL)),
            ("Scope", names(Scope::ALL)),
            ("ViewMode", names(ViewMode::ALL)),
            ("Nametags", NAMETAG_MODES.iter().map(|(n, _)| n.to_string()).collect()),
            ("Typeface", TYPEFACES.iter().map(|(n, _)| n.to_string()).collect()),
            ("Kind", names(wryme::md::Kind::ALL)),
            ("Notice", names(wryme::control::Notice::ALL)),
        ];
        for (swift_name, core) in pairs {
            assert_eq!(cases(&src, swift_name), core, "Swift enum {swift_name} differs from the core");
        }
    }

    #[test]
    fn swift_paints_every_colour_the_core_names() {
        use ratatui::style::Color::*;
        let src = swift();
        let named = [
            Black, Red, Green, Yellow, Blue, Magenta, Cyan, Gray, DarkGray, LightRed, LightGreen,
            LightYellow, LightBlue, LightMagenta, LightCyan, White,
        ];
        let missing: Vec<String> = named
            .iter()
            .map(|c| c.to_string())
            .filter(|n| !src.contains(&format!("\"{n}\":")))
            .collect();
        assert!(missing.is_empty(), "Swift has no colour for {missing:?}");
    }
}
