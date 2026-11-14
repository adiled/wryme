# Parity

One core drives every surface: the terminal (`src/term.rs`, `src/ui.rs`, `src/popup_ui.rs`), the browser (the same terminal surface on a web backend), and the Mac and iPhone apps (`apple/`).

## Shared, so it cannot drift

- `src/scene.rs` describes everything any surface shows. Painters read only the scene, never `App`.
- `src/action.rs` holds every shortcut with its scope and help text. The Help tab, terminal keys, Mac menus and key handling, and iPhone hardware-keyboard shortcuts all come from this one table.
- `src/control.rs` is what core actions do.

## Each surface implements

- `Edit` actions (the text field) and `Pane` actions (paging, copy). Rust matches them with no catch-all, so a new variant fails to compile until handled.
- `cargo test -p wryme-apple` fails if the Swift code has no case for any `Edit` or `Pane` action, or any scope, view mode, nametag mode or font.
- Swift decodes the scene strictly: anything it does not know shows an error screen instead of being dropped.

## Intentional differences

| Area | Terminal | Mac | iPhone |
|---|---|---|---|
| Chat text | the terminal's font | font pref (system, serif, rounded, mono) | font pref |
| Code, tables, settings rows | monospace | monospace | monospace |
| Dim grey | terminal palette | lighter `#7a7a88` | lighter `#7a7a88` |
| Selecting text | drag, copies on release; Shift-arrows extend | native selection, Cmd-C | native selection |
| Page mode | pins the start of a long reply, steps rows, then pages | snaps whole screens | snaps whole screens |
| Position readout | rows | percent | percent |
| Station search | type letters | type letters | search field |
| Images | send a path as the whole message | drop a file | photo picker |
| Tools | all | all | none that run programs (shell, explore, job check) |
| Quit (Ctrl-C) | quits | quits | not offered |
| Extras | | menu bar, Cmd-, settings | action menu |
| Sync row (Pref tab) | macOS: links the files into iCloud Drive › wryme; elsewhere hidden | same as the terminal on macOS | opens the Files folder picker |

## Sync

Shops, stations and prefs can live in a shared folder. The Pref tab has one `sync` row everywhere it can work, backed by `src/sync.rs`, in one of two ways:

- **Link**: the core moves the files into a cloud drive and leaves links behind. macOS (terminal and Mac app) uses iCloud Drive this way, since a terminal can't show a folder picker. Turning it off copies the files back and leaves the cloud copy for other devices.
- **Pick**: the core asks the surface to open its own folder picker by bumping `scene.sync.pick`. The iPhone uses the Files picker, the browser uses its directory picker, and Android will use its document picker, which reaches Google Drive.

The iPhone rereads the folder when the app comes to the front. Book memory and usage stats stay per device.

With sync on, the Mac writes its network name (`<name>.local`) as `host` in prefs.json. The iPhone swaps that name in for any shop on `localhost` or `127.0.0.1`, so local models are reached on the Mac. Those services must listen on the network, not just 127.0.0.1. Set `host` to another name, such as a Tailscale one, and the Mac leaves it alone.

## Checks

`cargo test --lib` · `cargo test -p wryme-apple` · `sh apple/build.sh mac` · `sh apple/build.sh ios-sim`
