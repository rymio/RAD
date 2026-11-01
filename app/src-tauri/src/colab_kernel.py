"""RAD's Python Colab kernel: one process per notebook, its variables kept from cell to cell.

Requests come one per line on stdin, {"id": 1, "op": "run", "code": "…"}; every answer is a line
of JSON on the protocol stream (the process's stdout, which nothing else may write to). A run
streams `stdout` and `stderr` while the cell runs, `display` for rich output (pictures, tables,
HTML), then ends with `done` (the value of the last expression, if any) or `error`.

- Like IPython: the last expression of a cell is shown (unless the cell ends in `;`); `display(x)` shows anything along the way;
  matplotlib figures are shown when the cell ends; `!command` runs a shell command and
  `%pip install …` installs into this notebook's Python.
- `%pip` uses the local wheelhouse (RAD_COLAB_WHEELS): online, packages are downloaded there first
  and installed from it, so they're there again when offline; offline (the RAD_COLAB_OFFLINE_FLAG
  file, kept up to date by RAD, exists), only what's in it can be installed.
- An interrupt is SIGINT, as in a terminal: the cell stops with KeyboardInterrupt.
- `vars` lists the notebook's variables with their types and a short preview.

What C code prints straight to file descriptor 1 goes to stderr, so it can't break the protocol.
"""

import ast
import base64
import builtins
import io
import json
import linecache
import os
import shlex
import signal
import subprocess
import sys
import threading
import time
import traceback
import types

PROTO = os.fdopen(os.dup(1), "w", buffering=1, encoding="utf-8")
os.dup2(2, 1)
LOCK = threading.Lock()
CURRENT = {"id": 0}


def send(msg):
    with LOCK:
        PROTO.write(json.dumps(msg, default=str) + "\n")
        PROTO.flush()


class Stream(io.TextIOBase):
    """sys.stdout / sys.stderr: what the cell prints, sent as it's printed."""

    def __init__(self, name):
        self.name = name

    def writable(self):
        return True

    def isatty(self):
        return False

    def write(self, s):
        if s:
            send({"id": CURRENT["id"], "type": self.name, "text": s})
        return len(s)

    def flush(self):
        pass


sys.stdout = Stream("stdout")
sys.stderr = Stream("stderr")


def no_input(prompt=""):
    raise RuntimeError("input() can't be used in a Colab cell: set the value in the code instead.")


builtins.input = no_input
os.environ.setdefault("MPLBACKEND", "Agg")
os.environ.setdefault("TQDM_DISABLE_COLOUR", "1")
sys.path.insert(0, os.getcwd())

# ---- rich output


def mime_bundle(obj):
    """What a value looks like, by MIME type (the formats Jupyter uses)."""
    data = {}
    for meth, mime in (("_repr_html_", "text/html"), ("_repr_svg_", "image/svg+xml"), ("_repr_markdown_", "text/markdown"), ("_repr_latex_", "text/latex")):
        f = getattr(obj, meth, None)
        if callable(f):
            try:
                v = f()
                if isinstance(v, tuple):
                    v = v[0]
                if v:
                    data[mime] = v
            except Exception:
                pass
    f = getattr(obj, "_repr_png_", None)
    if callable(f):
        try:
            v = f()
            if isinstance(v, tuple):
                v = v[0]
            if v:
                data["image/png"] = v if isinstance(v, str) else base64.b64encode(v).decode()
        except Exception:
            pass
    # PIL images
    if "image/png" not in data and type(obj).__module__.startswith("PIL.") and hasattr(obj, "save"):
        try:
            buf = io.BytesIO()
            obj.save(buf, format="PNG")
            data["image/png"] = base64.b64encode(buf.getvalue()).decode()
        except Exception:
            pass
    try:
        text = repr(obj)
    except Exception as e:
        text = f"<repr failed: {e}>"
    if len(text) > 20000:
        text = text[:20000] + " …"
    data["text/plain"] = text
    return data


def display(*objs, **_):
    for obj in objs:
        send({"id": CURRENT["id"], "type": "display", "data": mime_bundle(obj)})


class HTML:
    def __init__(self, html):
        self.html = html

    def _repr_html_(self):
        return self.html


class Markdown:
    def __init__(self, md):
        self.md = md

    def _repr_markdown_(self):
        return self.md


class Math:
    def __init__(self, tex):
        self.tex = tex

    def _repr_latex_(self):
        return f"$${self.tex}$$"


def show_figures():
    """Every open matplotlib figure, as a picture, then closed (as `%matplotlib inline` does)."""
    plt = sys.modules.get("matplotlib.pyplot")
    if plt is None:
        return
    try:
        for num in plt.get_fignums():
            fig = plt.figure(num)
            buf = io.BytesIO()
            fig.savefig(buf, format="png", bbox_inches="tight", dpi=110)
            send({"id": CURRENT["id"], "type": "display", "data": {"image/png": base64.b64encode(buf.getvalue()).decode(), "text/plain": f"<Figure {num}>"}})
        plt.close("all")
    except Exception as e:
        sys.stderr.write(f"Couldn't show the figure: {e}\n")


# ---- magics


def run_shell(cmd):
    p = subprocess.Popen(cmd, shell=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, bufsize=1)
    try:
        for line in p.stdout:
            sys.stdout.write(line)
        p.wait()
    except KeyboardInterrupt:
        p.terminate()
        raise
    if p.returncode:
        sys.stderr.write(f"(exit code {p.returncode})\n")
    return p.returncode


def pip(args):
    argv = shlex.split(args)
    if not argv:
        return
    wheels = os.environ.get("RAD_COLAB_WHEELS")
    flag = os.environ.get("RAD_COLAB_OFFLINE_FLAG")
    offline = os.environ.get("RAD_COLAB_OFFLINE") == "1" or bool(flag and os.path.exists(flag))
    py = shlex.quote(sys.executable)
    if argv[0] == "install" and wheels:
        names = " ".join(shlex.quote(a) for a in argv[1:])
        w = shlex.quote(wheels)
        if offline:
            sys.stdout.write("Offline: installing from the local packages.\n")
        elif run_shell(f"{py} -m pip download --disable-pip-version-check --progress-bar off -d {w} {names}") != 0:
            sys.stderr.write("Download failed; trying the local packages.\n")
        run_shell(f"{py} -m pip install --disable-pip-version-check --no-index --find-links {w} {names}")
    else:
        run_shell(f"{py} -m pip --disable-pip-version-check {' '.join(shlex.quote(a) for a in argv)}")
    sys.stdout.write("Restart the kernel if a package you'd already imported was upgraded.\n")


NS = {"__name__": "__main__", "__builtins__": builtins, "display": display, "HTML": HTML, "Markdown": Markdown, "Math": Math}
NS["get_ipython"] = lambda: None


def transform(code):
    """`!cmd`, `%pip …` and `%%`/`%` lines that mean nothing here become Python."""
    out = []
    for line in code.split("\n"):
        s = line.lstrip()
        indent = line[: len(line) - len(s)]
        if s.startswith("!"):
            out.append(f"{indent}__rad_shell({s[1:]!r})")
        elif s.startswith(("%pip ", "%pip\t", "pip install ")):
            out.append(f"{indent}__rad_pip({s.split(None, 1)[1] if s.startswith('%') else s[4:]!r})")
        elif s.startswith(("%matplotlib", "%load_ext", "%config")):
            out.append(f"{indent}pass")
        elif s.startswith("%time "):
            out.append(f"{indent}__rad_t0 = __import__('time').perf_counter(); {s[6:]}; print(f'Wall time: {{(__import__(\"time\").perf_counter() - __rad_t0) * 1000:.1f}} ms')")
        else:
            out.append(line)
    return "\n".join(out)


NS["__rad_shell"] = lambda cmd: run_shell(cmd) and None
NS["__rad_pip"] = pip

# ---- running a cell

COUNT = {"n": 0}


def run(code):
    COUNT["n"] += 1
    name = f"<cell {COUNT['n']}>"
    src = transform(code)
    linecache.cache[name] = (len(src), None, src.splitlines(True), name)
    tree = ast.parse(src, name, "exec")
    last = None
    # As in IPython, a cell ending in ";" shows no value.
    quiet = src.rstrip().endswith(";")
    if tree.body and isinstance(tree.body[-1], ast.Expr) and not quiet:
        last = ast.Expression(tree.body.pop().value)
    exec(compile(tree, name, "exec"), NS)
    value = eval(compile(last, name, "eval"), NS) if last is not None else None
    show_figures()
    if value is not None:
        NS["_"] = value
        return mime_bundle(value)
    return {}


def error_info(e, kind):
    tb = e.__traceback__
    # Only the notebook's own frames and the libraries below them, not this kernel's.
    while tb is not None and not tb.tb_frame.f_code.co_filename.startswith("<cell"):
        tb = tb.tb_next
    lines = traceback.format_exception(type(e), e, tb) if tb is not None or isinstance(e, SyntaxError) else traceback.format_exception_only(type(e), e)
    line = None
    if isinstance(e, SyntaxError):
        line = e.lineno
    else:
        for fs in traceback.extract_tb(e.__traceback__):
            if fs.filename.startswith("<cell"):
                line = fs.lineno
    return {"kind": kind, "ename": type(e).__name__, "message": str(e), "traceback": "".join(lines), "line": line}


def variables():
    out = []
    for k, v in list(NS.items()):
        if k.startswith("_") or k in ("display", "HTML", "Markdown", "Math", "get_ipython", "In", "Out"):
            continue
        # The data, not the modules, functions and classes.
        if isinstance(v, (types.ModuleType, types.FunctionType, types.BuiltinFunctionType, type)):
            continue
        t = type(v).__name__
        shape = getattr(v, "shape", None)
        try:
            preview = repr(v)
        except Exception:
            preview = "…"
        info = {"name": k, "type": t, "preview": preview[:120]}
        if shape is not None:
            info["shape"] = str(tuple(shape)) if not isinstance(shape, str) else shape
            dtype = getattr(v, "dtype", None)
            if dtype is not None:
                info["dtype"] = str(dtype)
        elif hasattr(v, "__len__") and t in ("list", "dict", "tuple", "set", "str"):
            info["len"] = len(v)
        out.append(info)
    return out


def main():
    signal.signal(signal.SIGINT, signal.default_int_handler)
    send({"type": "ready", "python": sys.version.split()[0], "executable": sys.executable})
    while True:
        try:
            line = sys.stdin.readline()
        except KeyboardInterrupt:
            # An interrupt that came just after the cell ended.
            continue
        if not line:
            break
        try:
            req = json.loads(line)
        except ValueError:
            continue
        rid = req.get("id", 0)
        op = req.get("op")
        if op == "run":
            CURRENT["id"] = rid
            t0 = time.perf_counter()
            try:
                data = run(req.get("code", ""))
                send({"id": rid, "type": "done", "data": data, "ms": int((time.perf_counter() - t0) * 1000)})
            except KeyboardInterrupt as e:
                show_figures()
                send({"id": rid, "type": "error", "ms": int((time.perf_counter() - t0) * 1000), **error_info(e, "interrupted"), "message": "Stopped (KeyboardInterrupt). The variables are kept."})
            except BaseException as e:  # noqa: BLE001 — SystemExit too: the notebook goes on
                show_figures()
                send({"id": rid, "type": "error", "ms": int((time.perf_counter() - t0) * 1000), **error_info(e, "syntax" if isinstance(e, SyntaxError) else "exception")})
        elif op == "vars":
            send({"id": rid, "type": "vars", "vars": variables()})
        else:
            send({"id": rid, "type": "error", "kind": "request", "message": f"unknown op {op}"})


if __name__ == "__main__":
    main()
