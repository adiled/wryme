https://github.com/user-attachments/assets/eaa35391-7219-41ea-8736-b928e5edf280


# wryme

(pronounced "write me", said quickly.)

wryme • that small, calm window where agents come to meet you!

You type your message at the top. The AI's reply appears right under what you
typed and grows downward as it writes. Your earlier conversations sit below
that, oldest at the bottom. No tabs, no menus, no surprises.

The little command you'll type to open it is `wme`.

## Try it before doing anything

You don't need a key, a login, or an internet connection to see what this
thing looks like. Just [open it in your browser](https://adiled.github.io/wryme/), or install it (below) and run `wme`. With nothing set up,
it answers in canned nonsense so you can poke around the screen and decide whether you want more.

## Installing it

```sh
cargo install wryme
```

or build yourself:

```sh
make build
make install
```

That puts a program called `wme` in your `~/.local/bin` folder. If that folder
isn't already on your PATH, your terminal won't find it; ask whoever set up
your computer to add it, or add this line to your `~/.zshrc` or `~/.bashrc`:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

## The two pieces: shops and stations

wryme uses two simple ideas.

- A **shop** is a place that runs AI for you. It has an internet address
  and (usually) a key. The shop lists which models it can run. Examples:
  OpenAI, a server in your garage, or device at hand.

- A **station** is the AI you actually talk to. It picks a model from a
  shop and sets a few knobs (how creative, how careful, how long it can
  talk). You can save as many stations as you want and pick which one you
  want when you launch.

At launch wryme finds a shop that runs the model your station asked for,
and away you go.

## Set up your first shop

Edit the file `~/.config/wryme/shops.toml` and put one block in it for the
AI service you have an account with:

💡 Default protocol is [open-responses](https://www.openresponses.org/), and chat-completions AKA OpenAI-Compat is also supported.

**Open AI itself**
```sh
export OPENAI_API_KEY="sk-...your-key..."
```

```toml
[[shop]]
name = "openai"
url = "https://api.openai.com/v1"
key_env = "OPENAI_API_KEY"
```

You can use `key_env` to point at any env var name, or `key = "..."` if
you want to put the key directly in the file.

**Grok**
```toml
[[shop]]
name = "groq"
url = "https://api.groq.com/openai/v1"
key_env = "GROQ_API_KEY"
models = ["llama-3.3-70b-versatile", "llama-3.3-8b-instant"]
```

**Ollama on your own computer. No key needed. Models auto-discover!**
```toml
[[shop]]
name = "ollama"
url = "http://localhost:11434/v1"
```

**Any other local runner, like antirez/ds4 (hint: this one runs better w/ chat-completions)**
```
[[shop]]
name = "ds41"
url = "http://127.0.0.1:8000/v1"
protocol = "chat-completions"
models = ["deepseek-v4.1-flash"]
```

With just one shop and station configured, you can simply `wme` in, and ask the assistant to configure shops and stations.

💡 **For agents:** The `models` list works as a filter: if shop auto discovery doesn't work, this list is used; And if the discovery does work, it limits down to configured list. List them newest-first; if you launch wryme without picking a station, it will use
the first model in the first shop.

## Make your stations

Make the file `~/.config/wryme/stations.toml`. Each station is a model
plus optional dials:

```toml
[[station]]
name = "quick chat"
model = "gpt-4o-mini"

[[station]]
name = "deep thinker"
model = "o1-mini"
patience = "slow"

[[station]]
name = "creative"
model = "gpt-4o"
boldness = 1.3
```

The station dials, all optional:

- **`boldness`**: how wild the AI gets. A number between 0 and 2. Try 0.3
  for safe, 0.7 for balanced, 1.3 for spicy. Unset means the AI decides.
- **`patience`**: how hard the AI thinks before answering. `quick`,
  `steady`, or `slow`. Only matters on models that support extended
  thinking. Unset means the AI decides.
- **`brainy`**: how much of that thinking gets shown. `hush` (show none at
  all), `murmur`, `chatty`, or `gabby`. Unset means the AI decides. A short
  retelling of the thinking prints under **brain**; the thinking itself, on
  models that hand it over, prints under **heart**.
- **`tinker_keep`**: how many tool pairs survive in replay. `all` (default),
  a positive integer (`8`, `3`), `0` (keep none), or a percent (`50%`). Keeps the last N pairs, pair stays atomic.
- **`tinker_clip`**: how much of each kept tool result body survives. `all` (default, verbatim), a positive integer chars (`500`, `1000`), `0` (empty body keeps `call_id`), or a percent (`50%` of original). Applies to replayed history, not the turn in progress.
- **`tinker_depth`**: how many rounds of tool use one turn may take before wryme forces an answer. `all` (default, unbounded), or a positive integer (`8`). At the limit the agent gets one more request with tools withheld and a nudge to write up what it has. Bounds latency and context growth; a tool-hungry model otherwise re-sends the whole conversation every round, getting slower each time with nothing printed.
- **`voice`**: speech voice for read-aloud assistant. Any `say` voice on macOS
  (`Tara` at 260wpm when unset), any espeak voice on Linux. Needs `say`/`spd-say`
  on PATH; silently off otherwise.

## Usage preferences

These are saved in `~/.config/wryme/prefs.json`. Change them from the
**preferences** rows in the Ctrl-S popup, or edit the file:

```json
{
  "accent": "cyan",
  "nametags": "inlined"
}
```

- **`accent`**: the highlight color. `cyan` (default), `azure`, `green`,
  `lightgreen`, `yellow`, `magenta`, `red`, `blue`, `white`, `gray`,
  `lightcyan`, `lightyellow`, or `lightmagenta`.
- **`nametags`**: how messages are labeled. `standout` (default) puts a
  `you` / `assistant` header above each message. `inlined` drops the header:
  a small icon sits on the left and the time on the right, with the text
  wrapped between them. `⏹` marks your messages. The assistant's icon shows
  what it's doing and blinks while busy: `⬩` writing, `✦` thinking, `‣`
  using a tool, `಄` reminiscing, `⌃` done. Thinking text is marked `✶`
  (brain) or `❤︎` (heart). A message you send while a reply is still coming
  in shows dimmed with `waiting…` until it goes out.

These reset every session:

**`view mode`**: Ctrl+T to toggle line-by-line vs pagination based scrolling.

**`voice toggle`**: Ctrl+V to toggle assistant voiceover whenever needed.

## Using it

Type in terminal or desktop app search:

```sh
wme
```

The window opens. The blinking cursor is in a box at the top. Write away! Press **Enter**. The answer streams in below.

Want quick response?

```sh
wme -p "why is wryme's pet name wme?" --station "demo"
```

When you're done, press **Ctrl-C** to close it.

### Keys you might want to know

| Press        | What happens                                  |
|--------------|-----------------------------------------------|
| `Enter`      | Send what you typed.                          |
| `Esc`        | Stop a reply that's still coming in.          |
| `Ctrl-C`     | Close the program.                            |
| `Ctrl-T`     | Switch between paged and scrolling view.      |
| `Ctrl-V`     | Read replies aloud on / off.                  |
| `Ctrl-S`     | Open the station popup (a tabbed tuning menu).|
| `F1`         | Open the popup directly on the Help tab.      |
| `Tab`        | In the popup: switch between Station / Help.  |
| `PgUp/PgDn`  | Scroll the popup body (also the mouse wheel). |
| arrow keys   | Move the cursor inside your typing.           |
| `Backspace`  | Delete the character before the cursor.       |

### Assistants talk to each other

Every open wryme shows its id in the status bar, like `wryme#72e8`. Ask one
window to "ask wryme#72e8 what time it is" and its assistant sends a note.
The other window answers on its own, and the answer comes back to you.
Arriving notes and answers show up as `guest@72e8`. Web app assistants can
participate if at least one native window is open.

## What's cooking?

Wryme has both, web and desktop clients, besides TUI. All UX is consistent.

- For desktop, signed OSX releases are planned.
- For desktop, more multimedia support is planned.
- For web, memory and shell / machine use capability is planned. (besides through hive)
- [hum](https://adiled.github.io/hum) hive support is set out to make the web version extremely powerful.

## Trouble

- **`wme: command not found`**: your `~/.local/bin` or `~/.cargo/bin` isn't on your PATH. See
  the install section above.
- **The status bar says `station: demo`**: you haven't set up a shop yet.
  See "Set up your first shop" above.
- **The status bar says `unavailable: X`**: no shop lists station X's model,
  so wryme started on the first station that works instead. Add the model to
  a shop's `models` list or fix the spelling. Asking for X by name
  (`--station X`) fails with `station 'X' wants model 'Y' but no shop
  advertises it`.
- **In the browser, a local shop's models are missing**: a page can only
  read a local server that allows cross-origin requests. The server must
  send `Access-Control-Allow-Origin` and answer `OPTIONS` preflight
  requests. Ollama allows local pages by default.
- **`upstream 401`**: the key for that shop is wrong, missing, or expired.
- **`upstream 404`**: the model name is wrong, or the shop URL is wrong.

## License

MIT. See `LICENSE`.
