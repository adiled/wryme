# wryme

(pronounced "write me", said quickly.)

A small, calm window in your terminal where you can chat with an AI.

You type your message at the top. The AI's reply appears right under what you
typed and grows downward as it writes. Your earlier conversations sit below
that, oldest at the bottom. No tabs, no menus, no surprises.

The little command you'll type to open it is `wme`.

## Try it before doing anything

You don't need a key, a login, or an internet connection to see what this
thing looks like. Just install it (below) and run `wme`. With nothing set up,
it answers in canned nonsense so you can poke around the screen and decide
whether you want it.

## Installing it

In your terminal, run:

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

## Stations

The thing on the other end of the conversation is a **station**. A station is
just a name plus an internet address, a model, and (usually) a secret key.
You pick one and you talk to it. That's all there is to it.

Three ways you can have stations:

1. **`demo`** — always there. Built in. Streams canned nonsense. Zero setup.
2. **The default station** — one you set up once in your `~/.zshrc` so it's
   ready every time you launch.
3. **More stations** — a list in `~/.config/wryme/stations.toml`. Useful if
   you switch between providers.

### Set up your default station

Pick whichever AI service you have an account with and paste the matching
block into your `~/.zshrc` or `~/.bashrc`:

```sh
# OpenAI
export WME_DEFAULT_STATION_NAME="openai"
export WME_DEFAULT_STATION_URL="https://api.openai.com/v1"
export WME_DEFAULT_STATION_KEY="sk-...your-key-goes-here..."
export WME_DEFAULT_STATION_MODEL="gpt-4o-mini"
```

```sh
# Groq
export WME_DEFAULT_STATION_NAME="groq"
export WME_DEFAULT_STATION_URL="https://api.groq.com/openai/v1"
export WME_DEFAULT_STATION_KEY="gsk-...your-key..."
export WME_DEFAULT_STATION_MODEL="llama-3.3-70b-versatile"
```

```sh
# Ollama on your own computer — no key needed
export WME_DEFAULT_STATION_NAME="local"
export WME_DEFAULT_STATION_URL="http://localhost:11434/v1"
export WME_DEFAULT_STATION_MODEL="llama3"
```

Where do you get a key? OpenAI: <https://platform.openai.com/api-keys>. Groq:
<https://console.groq.com/keys>. Each service has its own page.

(Shortcut: if you already have `OPENAI_API_KEY` in your environment, wryme
will use that as the key for the default station automatically. You still
need to set the URL and model if you want anything other than OpenAI's
defaults.)

### Adding more stations

Make the file `~/.config/wryme/stations.toml` and write each station as a
block:

```toml
[[station]]
name = "groq fast"
url = "https://api.groq.com/openai/v1"
model = "llama-3.3-70b-versatile"
key_env = "GROQ_API_KEY"           # reads $GROQ_API_KEY from your shell

[[station]]
name = "openai mini"
url = "https://api.openai.com/v1"
model = "gpt-4o-mini"
key = "sk-..."                     # or paste the key directly

[[station]]
name = "local"
url = "http://localhost:11434/v1"
model = "llama3"                   # no key field — Ollama doesn't need one
```

Then to launch with a specific station:

```sh
wme --station "groq fast"
```

Without `--station`, wryme uses your default (the env one), or the first one
in the config file if you don't have a default, or `demo` if you have
nothing at all.

## Using it

Type:

```sh
wme
```

The window opens. The blinking cursor is in a box at the top. Type whatever
you want to ask. Press **Enter**. The answer streams in below.

When you're done, press **Ctrl-C** to close it.

### Keys you might want to know

| Press        | What happens                                  |
|--------------|-----------------------------------------------|
| `Enter`      | Send what you typed.                          |
| `Esc`        | Stop a reply that's still coming in.          |
| `Ctrl-C`     | Close the program.                            |
| arrow keys   | Move the cursor inside your typing.           |
| `Backspace`  | Delete the character before the cursor.       |

That's the whole thing.

## Trouble

- **`wme: command not found`** — your `~/.local/bin` isn't on your PATH. See
  the install section above.
- **The status bar says `station: demo`** — you haven't set up a real one yet.
  See "Set up your default station" above.
- **`upstream 401`** — the key for that station is wrong, missing, or expired.
- **`upstream 404`** — the model name is wrong, or the URL is wrong.
- **`no station named '...'`** — the name passed to `--station` doesn't match
  any of the stations wryme could find. Check `~/.config/wryme/stations.toml`.

## License

MIT. See `LICENSE`.
