# writeme

A small, calm window in your terminal where you can chat with an AI.

You type your message at the top. The AI's reply appears right under what you
typed and grows downward as it writes. Your earlier conversations sit below
that, oldest at the bottom. No tabs, no menus, no surprises.

The little command you'll type to open it is `wme`.

## What you need

- A terminal (the black-and-white text window on your computer).
- An "API key" — a long secret password from an AI company that lets the
  program talk to their AI on your behalf. The most common one is from
  OpenAI: <https://platform.openai.com/api-keys>.
- About a minute.

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

## Telling it your key

Once, before you start it, paste your API key into the terminal like this:

```sh
export OPENAI_API_KEY="sk-...your-key-goes-here..."
```

If you want this to stick around so you don't retype it every time, add that
same line to your `~/.zshrc` or `~/.bashrc`.

## Using it

Type:

```sh
wme
```

The window opens. The blinking cursor is in a box at the top. Type whatever
you want to ask. Press **Enter**. The answer streams in below.

Type another thing. Press Enter. And so on.

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

## If you want to use a different AI

`wme` talks to anything that follows the same protocol OpenAI uses — which is
nearly all of them these days. You point it at a different "base URL":

```sh
# Run a model on your own computer with Ollama:
OPENAI_BASE_URL=http://localhost:11434/v1 WRITEME_MODEL=llama3 wme

# Use Groq instead of OpenAI:
OPENAI_BASE_URL=https://api.groq.com/openai/v1 \
  OPENAI_API_KEY=gsk-... \
  WRITEME_MODEL=llama-3.3-70b-versatile \
  wme
```

If you only ever use one, set those once in your `~/.zshrc` and forget them.

## Trouble

- **`wme: command not found`** — your `~/.local/bin` isn't on your PATH. See
  the install section above.
- **`upstream 401`** — the API key is wrong, missing, or expired.
- **`upstream 404`** — the model name is wrong, or the base URL is wrong.
- **Nothing happens when I press Enter** — make sure you actually typed
  something; an empty message is ignored on purpose.

## License

MIT. See `LICENSE`.
