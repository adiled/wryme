---
title: "wryme-hive (Rust)"
description: "wryme's tool surface as a hum forager bee — the TUI's real login shell (run/check/explore) and its book engine, exposed over thrum as tools humd can route to"
---

# wryme-hive

> _wryme's tool surface, commissioned as a hum bee — the same shell the
> terminal runs, now reachable by any worker in the nest_

wryme-hive is a **forager**: no LLM compute, no shops. It dials humd's
thrum socket, says hello with `bee: ["forager"]`, and advertises the
four tools wryme already ships:

| tool | what it does |
|---|---|
| `<shell>` (`zsh`, `bash`, ...) | run a command in the user's real login shell; >10s goes async (`gone async · id=N`) |
| `<shell>_check` | poll an async job's progress / final result |
| `<shell>_explore` | discovery pass — CSV of guessed words, exact tool names back |
| `book` | the Book: structured logs, citations, lineage, weave/diff/history |

Everything runs through `wryme::tools::execute` — literally the code
path the TUI uses. The dispatcher is a thin wire-shape adapter, so any
tool added to the TUI appears here for free.

## Architecture

Same shape as humfs (foragers calling foragers, humd routes by
`toolName`):

```
nestler (OC, Claude Code, ...) ─chi:tool-call(zsh)─► humd
   ─route by toolName─► wryme-hive (this bee) ─wryme::tools─► login shell
   → chi:"tool-result" back through the chain
```

`provides: ["shell"]` — humd uses the hive-level claim to deauthorize
the same shell surface from other sources, so nestler-declared shell
tools don't shadow ours.

## Book scoping

The hive keeps its own book at `~/.config/wryme/hive/book`, separate
from the TUI's `~/.config/wryme/book` — two processes never write the
same stream segments.

## Install

```sh
hum hive /path/to/wryme/hive install    # cargo install --locked → ~/.local/bin/wryme-hive, orchd up
hum bee --list                          # wryme-hive · in nest
```

## Status

Live: full tool surface, async job polling, book read/write through the
hive. Deferred alongside the TUI: IME/paste fidelity, hosted shops
(provider-executed tools — the Responses API `mcp` leg lands upstream
in wryme first).
