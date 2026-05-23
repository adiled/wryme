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

## The two pieces: shops and stations

wryme uses two simple ideas.

- A **shop** is a place that runs AI for you. It has an internet address
  and (usually) a key. The shop lists which models it can run. Examples:
  OpenAI, a server in your garage, a free local one.

- A **station** is the AI you actually talk to. It picks a model from a
  shop and sets a few knobs (how creative, how careful, how long it can
  talk). You can save as many stations as you want and pick which one you
  want when you launch.

At launch wryme finds a shop that runs the model your station asked for,
and away you go.

## Set up your first shop

Make the file `~/.config/wryme/shops.toml` and put one block in it for the
AI service you have an account with:

```toml
# OpenAI
[[shop]]
name = "openai"
url = "https://api.openai.com/v1"
key_env = "OPENAI_API_KEY"
protocol = "responses"
models = ["gpt-4o-mini", "o1-mini", "gpt-4o"]
```

```toml
# Groq
[[shop]]
name = "groq"
url = "https://api.groq.com/openai/v1"
key_env = "GROQ_API_KEY"
models = ["llama-3.3-70b-versatile", "llama-3.3-8b-instant"]
```

```toml
# Ollama on your own computer. No key needed.
[[shop]]
name = "local"
url = "http://localhost:11434/v1"
models = ["llama3", "qwen2.5"]
```

The `models` list is what the shop knows how to run. List them
newest-first; if you launch wryme without picking a station, it will use
the first model in the first shop.

Set your API key once in your `~/.zshrc` so wryme can read it:

```sh
export OPENAI_API_KEY="sk-...your-key..."
```

You can use `key_env` to point at any env var name, or `key = "..."` if
you want to put the key directly in the file.

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

The dials, all optional:

- **`boldness`**: how wild the AI gets. A number between 0 and 2. Try 0.3
  for safe, 0.7 for balanced, 1.3 for spicy. Unset means the AI decides.
- **`patience`**: how hard the AI thinks before answering. `quick`,
  `steady`, or `slow`. Only matters on models that support extended
  thinking. Unset means the AI decides.
- **`verbosity`**: the most words the AI is allowed to say. A number like
  1024 or 4096. Unset means no cap; the AI stops when it thinks it is done.

## Using it

Type:

```sh
wme
```

The window opens. The blinking cursor is in a box at the top. Type whatever
you want to ask. Press **Enter**. The answer streams in below.

Want a specific station? Pass its name:

```sh
wme --station "deep thinker"
```

When you're done, press **Ctrl-C** to close it.

### Keys you might want to know

| Press        | What happens                                  |
|--------------|-----------------------------------------------|
| `Enter`      | Send what you typed.                          |
| `Esc`        | Stop a reply that's still coming in.          |
| `Ctrl-C`     | Close the program.                            |
| `Ctrl-T`     | Switch between paged and scrolling view.      |
| arrow keys   | Move the cursor inside your typing.           |
| `Backspace`  | Delete the character before the cursor.       |

## Trouble

- **`wme: command not found`**: your `~/.local/bin` isn't on your PATH. See
  the install section above.
- **The status bar says `station: demo`**: you haven't set up a shop yet.
  See "Set up your first shop" above.
- **`station 'X' wants model 'Y' but no shop advertises it`**: the station's
  model name doesn't match any of your shops' `models` lists. Add it to a
  shop or fix the spelling.
- **`upstream 401`**: the key for that shop is wrong, missing, or expired.
- **`upstream 404`**: the model name is wrong, or the URL is wrong.

## License

MIT. See `LICENSE`.
