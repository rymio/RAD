# RymScript SDK

RymScript is Rymnet's app language: Python syntax, running on the
already-embedded RustPython interpreter (the same one `rymsh`'s `python`
command has used since Milestone 11), restricted to a small, curated API.
It's not a general-purpose Python distribution — there's no real
filesystem/network/process access, and only a hand-picked slice of the
standard library is importable (see below) — but it's no longer
import-free either: `math`, hashing, `random`, `struct`, `csv`, and a
few others are available to any `.run` or `.win` program. What you get
is a well-defined, sandboxed contract for writing two kinds of small
program, plus a runtime for each built specifically for it.

Why a Python dialect instead of a from-scratch language: RustPython is
mature, already proven in this codebase, and inventing a new parser from
scratch is a huge amount of surface area to get right (and keep right)
for what this needs. What's actually novel here — and what makes this
"RymScript" rather than "you can run arbitrary Python" — is the curated,
sandboxed API surface and the `.win` declarative-UI contract described
below, neither of which exist in stock Python.

## Programs folder

Every account gets a `/Programs` folder in its virtual computer,
auto-created the first time that account logs in (from `rymsh` or
`rymwin`, whichever comes first), seeded with two examples:
`Programs/hello.run` and `Programs/counter.win`. Put your own programs
there. Nothing enforces the location — RymScript will run a file from
anywhere in your vfs — but `/Programs` is the convention both runtimes
and the file tree UIs assume.

## Two program types

**`*.run` — terminal programs.** Run from `rymsh`:

```text
rymsh:/home$ run hello
```

`run <name>` looks for `/Programs/<name>.run`. These are plain
sandboxed Python scripts, executed top to bottom, exactly like `rymsh`'s
existing `python <file>` command (in fact `run` is a thin wrapper around
it, pointed at `/Programs`). `print()` and `input()` work like normal
Python, since a terminal is right there. If you try to `run` a `.win`
file by name, `rymsh` tells you to open it from `rymwin`/Rymcode instead
of failing with a confusing "no such file".

**`*.win` — desktop programs.** Run only inside `rymwin` (the graphical
desktop) — launched from Rymcode's Run button, or opened directly from
File Manager. There is no terminal for a `.win` program to print to or
read input from; instead it declares its UI every frame and reacts to
what the user did last frame. See the widget contract below.

## The `.win` widget contract

A `.win` script defines one function:

```python
def draw(events):
    ...
    return [ ...list of widget dicts... ]
```

`rymwin` calls `draw(events)` once per rendered frame and draws
whatever list of widget dicts it returns, top to bottom, in that
program's window. Module-level variables persist across frames exactly
like a normal long-running program's state would — there's one
interpreter per running `.win` window, kept alive for as long as that
window stays open, not reset each frame.

`events` is a dict describing what happened to each *named* widget
(widgets you gave an `"id"`) since the last frame:

- a **button**'s id is present (`True`) for exactly one frame, the frame
  after it was clicked
- a **text_input**'s id holds its current string value, present every
  frame from the first time that widget is drawn onward
- a **checkbox**'s id holds its current bool value, same as text_input

Look things up with `events.get("some_id")` (returns `None` if that
widget hasn't been interacted with / drawn yet), not `events["some_id"]`.

### Widget dicts

| Type | Fields | Notes |
|---|---|---|
| `{"type": "heading", "text": str}` | | Bigger label. |
| `{"type": "label", "text": str}` | | Plain text. |
| `{"type": "separator"}` | | Horizontal rule. |
| `{"type": "button", "id": str, "text": str, "color": [r,g,b], "enabled": bool}` | `color`/`enabled` optional | `events[id]` is `True` the frame after a click. `color` is a `[r, g, b]` list of 0-255 ints (omit for the default look); `enabled` defaults to `True` -- a disabled button can't be clicked, useful for e.g. a Minesweeper cell that's already been revealed. |
| `{"type": "text_input", "id": str, "value": str}` | | `value` only seeds the field the *first* time this id is drawn — after that, whatever the user typed wins, and `value` in later dicts is ignored. Read the live text back via `events[id]`. |
| `{"type": "checkbox", "id": str, "text": str, "value": bool}` | | Same seed-once rule as text_input. |
| `{"type": "row", "children": [...]}` | | Lays its children out left-to-right instead of the default top-to-bottom stack. |
| `{"type": "column", "children": [...]}` | | An explicit vertical stack -- mostly useful nested inside a `row` (a `row` of `column`s gives you side-by-side panes). |
| `{"type": "grid", "id": str, "columns": int, "children": [...]}` | `id` required | A fixed-column-count grid, wrapping to a new row every `columns` children -- what a calculator keypad or a Minesweeper board is built from. `id` is required (unlike `row`/`column`) because the underlying grid widget needs a stable identity across frames. |

`row`/`column`/`grid` can nest arbitrarily (a `grid` of `row`s of buttons, etc.) -- `children` is just another list of widget dicts, parsed the same recursive way the top-level list is.

Unknown widget types, or a `draw()` that doesn't return a list of
dicts, raise a Python exception that's shown in the app's window instead
of crashing `rymwin` — you'll see the traceback right there and can fix
the script and re-run it.

### Example: a counter

```python
count = 0

def draw(events):
    global count
    if events.get("inc"):
        count += 1
    if events.get("reset"):
        count = 0
    return [
        {"type": "heading", "text": "Rymnet Counter"},
        {"type": "label", "text": f"Count: {count}"},
        {"type": "separator"},
        {"type": "button", "id": "inc", "text": "Increment"},
        {"type": "button", "id": "reset", "text": "Reset"},
    ]
```

This is `Programs/counter.win`, the starter example every new account
gets. It's covered by an automated test (`rymscript::tests::
counter_starter_program_runs_and_reacts_to_clicks` in `crates/rymwin/
src/rymscript.rs`) that drives it through several frames and checks the
count actually increments and resets — worth reading if you want to see
the contract exercised from the Rust side.

## File API (both program types)

```text
read_file(path) -> str
write_file(path, text)
append_file(path, text)
list_dir(path=".") -> list[str]
exists(path) -> bool
remove_file(path)
mkdir(path)
```

Paths are sandboxed to your own virtual computer, the same clamping
every other part of Rymnet uses: a leading `/` is root-relative,
anything else resolves against wherever the program considers "here"
(for `.run` scripts, your shell's current directory when you typed
`run`; for `.win` scripts, always the vfs root, since there's no
concept of a working directory in a desktop app). There is no way to
name a path outside your own account's storage — no real filesystem,
network, or process access exists from either program type.

## Time (both program types)

```text
now() -> str        # current local time as "HH:MM:SS"
now_ms() -> int      # current time as epoch milliseconds
```

There's no `import time`/`import datetime` — see "Why not the rest of
the stdlib" below for why RustPython can't really provide either — so
wall-clock access is two hand-implemented functions instead, the same
naming convention as the file API and `json_loads`/`json_dumps`. `now()`
covers "display the current time" (a clock); `now_ms()` covers "measure
elapsed time" (a stopwatch/timer) by taking the difference between two
readings yourself, e.g. `elapsed_seconds = (now_ms() - start_ms) // 1000`.

For a `.win` program specifically, `rymwin` also schedules a repaint
roughly once a second for every open window, so a script using `now()`
in its `draw()` output keeps visibly ticking even while nobody's
touching the window — it isn't limited to redrawing only in response to
clicks/typing the way earlier versions of the widget contract were.

## Standard library

Both program types share one curated interpreter (`build_interpreter()`
in `crates/rymwin/src/rymscript.rs`, duplicated in `crates/rymsh/src/
python.rs` for the same reasons as everything else in that crate). It's
built by taking `rustpython-stdlib`'s native (Rust-implemented) modules
and keeping only a small allowlist — everything that can touch the real
host (`os`, `socket`, `subprocess`, `mmap`, `select`, `ctypes`, and every
other module gated behind that crate's own `host_env` feature) is not
just unregistered, it's not even compiled into the binary, since this
workspace builds `rustpython-stdlib` with `host_env` disabled.

Importable today:

```python
import math
import cmath
import unicodedata
import zlib
import binascii
import csv
import struct
import random
import md5, sha1, sha256, sha3, sha512, blake2

import json_loads, json_dumps   # not real modules -- see below
```

`math`, `cmath`, `unicodedata`, `zlib`, `binascii`, and `struct` behave
exactly like real Python — full API, normal calling convention. Three
groups don't:

- **`random`** is RustPython's low-level accelerator, not the real
  `random` module, so there's no module-level `random.random()`. Make an
  instance first: `random.Random().random()`. That instance only has
  `.random()`, `.getrandbits(k)`, and `.seed()`/`.getstate()`/
  `.setstate()` — no `.randint(a, b)`, no `.choice(seq)`. Build what you
  need on top of `.random()` (e.g. a random digit is `int(rng.random() *
  10)`), the way `checksum_tool.run` does.
- **Hashing is one small module per algorithm**, not a unified
  `hashlib` dispatcher: `sha256.sha256(data)`, `md5.md5(data)`,
  `sha1.sha1(data)`, `sha3.sha3_256(data)` (and `sha3_224`/`_384`/`_512`),
  `sha512.sha512(data)`, `blake2.blake2b(data)` (and `blake2s`). Each
  returns a hasher object with the normal `.hexdigest()`/`.digest()`/
  `.update()` methods.
- **`json` isn't a module at all.** RustPython's native `_json`
  accelerator has no `.loads`/`.dumps` — those only exist in real
  CPython's pure-Python `json` package, which isn't available here (see
  "Why not the rest of the stdlib" below). Instead there are two plain
  global functions, alongside the file helpers: `json_loads(text) ->
  obj` and `json_dumps(obj) -> str`, hand-implemented in Rust against
  `serde_json`. Same naming convention as `read_file`/`write_file` —
  functions, not `import json`.

`csv` works like real Python (`csv.writer(fileobj)`, `csv.reader(...)`,
`.writerow(...)`) once you know the default "excel" dialect had to be
registered by hand at interpreter startup — RustPython's native `_csv`
doesn't come with one built in either (same underlying gap as `json`),
and calling `csv.writer()`/`csv.reader()` with no dialect before that
fix crashed the whole process, not just the script. That fix already
ships; it's mentioned here only because it's the kind of native-module
gap worth knowing exists if `csv` ever misbehaves in a way that looks
like a missing default.

### Why not the rest of the stdlib

A wider set of modules exist as native Rust code in `rustpython-stdlib`
but aren't usable even so: `json`, `bisect`, `statistics`, `bz2`, and
`lzma` register under CPython's internal accelerator name (`_json`,
`_bisect`, ...) because in real CPython the friendly public module is a
thin **pure-Python wrapper** around that accelerator, and RustPython
doesn't ship that wrapper — it would mean vendoring actual `.py` stdlib
source and using RustPython's frozen-module build step, a materially
bigger, separate undertaking. Without it, these accelerators only expose
low-level primitives, not the functions a script would actually call
(confirmed by writing a probe script and calling every candidate
function, not by reading the source — several looked plausible from
their names and weren't). `re` and `array` are absent for the same
reason (`array` also transitively needs `collections.abc`).

This is a deliberate, documented boundary, not an oversight — expanding
it further means building real frozen-module support, which is a
bigger project than a curated allowlist.

### Worked examples: a `.run` and a `.win` app using the stdlib

Every new account's `/Programs` folder ships two stdlib-flavored
examples alongside the plain `hello.run`/`counter.win` starters, so
there's real, runnable code to open rather than just this doc:

**`checksum_tool.run`** (terminal) — asks for a file, prints its
`sha256` checksum, generates a random confirmation code by hand (since
`random.Random()` has no `.randint`), and appends a row to a `csv` log
each time it runs. Exercises `sha256`, `random`, and `csv` together in
one program, including the `write`-method trick `csv.writer` needs
since there's no real file handle to hand it — build the row in memory
against a small buffer object, then hand the result to `write_file`/
`append_file`. Covered end-to-end by `terminal::tests::
checksum_tool_starter_program_runs_end_to_end` in `crates/rymwin/src/
terminal.rs`, which drives it through the same interactive `input()`
protocol the real Terminal app uses and checks the logged checksum and
csv row are correct.

**`notes.win`** (desktop) — a small checklist app that stores notes as
a JSON array (`[{"text": str, "done": bool}, ...]`) via `json_loads`/
`json_dumps`, instead of inventing a text format. Also demonstrates a
non-obvious `.win` technique: since a `text_input`'s `value` only seeds
the field the *first* time its id is drawn (see the widget contract
above), there's no way to clear it by re-sending `value=""` after a
note is added — so the script bumps the field's id (`draft_0`,
`draft_1`, ...) each time, which forces a fresh, empty widget next
frame. Covered by `rymscript::tests::
notes_starter_program_runs_and_persists_across_frames` in
`crates/rymwin/src/rymscript.rs`.

### Worked examples: `row`/`grid`, button styling, and time

Three more `.win` examples ship in `/Programs` specifically to prove the
`row`/`grid` layout containers, button `color`/`enabled`, and
`now`/`now_ms` are actually enough to build something real, not just
forms — each is covered end-to-end by its own test in
`crates/rymwin/src/rymscript.rs`, driving it through several frames the
same way `counter_starter_program_runs_and_reacts_to_clicks` does:

**`clock.win`** — a digital clock (`now()`, ticking on its own via the
once-a-second repaint mentioned above) plus a small stopwatch built on
`now_ms()`, laid out with a `row` for its Start/Reset buttons. Covered
by `clock_starter_program_ticks_and_runs_a_stopwatch`.

**`calculator.win`** — a standard four-function calculator, its keypad
a 4-column `grid` of buttons instead of one long vertical stack, with
`color` telling operator/clear/equals keys apart from digit keys at a
glance. Covered by `calculator_starter_program_computes_seven_plus_three`.

**`minesweeper.win`** — a small Minesweeper, its board an 8×8 `grid` of
per-cell buttons whose `text`/`color`/`enabled` reflect reveal state.
There's no right-click in the widget contract (a button only ever
reports a plain click), so flagging works through a "Flag mode"
checkbox instead — left-click either reveals or toggles a flag depending
on whether it's checked. Mine placement uses `random.Random()`, the
same non-standard calling convention `checksum_tool.run` already
demonstrates. Covered by
`minesweeper_starter_program_renders_a_full_board_and_flags_without_revealing`,
which checks the board renders all 64 cells hidden, that flag mode
flags without revealing, and that restart clears the board — not the
mine positions themselves, since those are genuinely random each run.

## Known v1 limitations

- **No custom drawing.** `row`/`column`/`grid` (see the widget contract
  above) cover side-by-side and grid layouts of the existing widget
  types, but there's still no canvas/pixel access, no images, and no
  positioning beyond what those three containers give you — enough for
  forms, keypads, and grid-based games, not for freeform graphics yet.
- **Single file, no user modules.** You can `import` from the curated
  stdlib above, but there's no module system for splitting your *own*
  code across files — everything a program needs lives in one
  `.py`-syntax file.
- **Rymcode's Run button for `*.run` files is a non-interactive
  preview.** It captures `print()` output into a small pane inside the
  editor, but `input()` raises immediately rather than hanging — there's
  no terminal inside a GUI text editor to read from. Test interactive
  `.run` programs with `rymsh`'s `run` command instead.
- **No package manager, no third-party libraries.** RymScript is
  intentionally small; if you need something not listed above, it isn't
  available.

These are honest v1 gaps, not permanent ones — the widget contract, the
file API, and the `RymApp` trait `.win` programs run on top of
(`crates/rymwin/src/app.rs`) were all designed to be extended rather
than replaced. The `row`/`column`/`grid` layout containers documented
above are exactly that kind of additive change (nested layout used to be
on this list too), and so is Rymcode's Design view -- a visual,
drag-and-drop-free form designer for `.win` apps, covered in
`docks/INSTALL_AND_CLI_GUIDE.md`'s "Design view" section. A
syntax-highlighting Code view, a canvas/drawing widget, or a
shared-module system are the same kind of thing, not yet built.
