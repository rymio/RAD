#!/usr/bin/env python3
"""rad-browse: a bot's hands in Chromium, inside its desktop VM (see containers.rs).

RAD runs `python3 /opt/rad/rad-browse.py <base64 JSON request>` and reads one JSON line back.
It talks to Chromium over the DevTools protocol on 127.0.0.1:9222, which only exists inside the
VM. Elements are numbered by the snapshot (a data-rad-id attribute on the page), so later calls
can point at them; nothing is kept between calls except which tab is current.

  {"op":"snapshot"}                         -> the page: url, title, text, numbered elements
  {"op":"goto","url":"https://…"}           {"op":"back"} {"op":"forward"} {"op":"reload"}
  {"op":"click","id":12}                    a real mouse click in the middle of the element
  {"op":"type","id":12,"text":"…","enter":true}   replaces the field's text
  {"op":"select","id":12,"value":"…"}       picks an option by its text or value
  {"op":"key","key":"Enter"}                Enter, Tab, Escape, Backspace, arrows, PageUp/Down…
  {"op":"scroll","direction":"down"}        about one screen
  {"op":"tabs"} {"op":"tab","index":1} {"op":"close_tab"}
  {"op":"dialog","accept":true}             answers a pop-up left open (rare: see below)
  {"op":"screenshot","path":"/root/shared/x.jpg"}  the browser's viewport (JPEG)
  {"op":"mouse","x":100,"y":200}            a click at viewport coordinates (the owner taking over)
  {"op":"text","text":"…"}                  typed into whatever has focus
  {"op":"at","x":100,"y":200}               what's at that point (for recording a demonstration)
  {"op":"fill_secret","id":12,"file":"/dev/shm/…","domain":"github.com"}
        types a secret that RAD wrote to a file in memory (the file is deleted at once), only
        if the page is on that domain. The secret is never printed.

Pop-ups (alert, confirm, prompt) are answered as soon as they open, in the same call: Chrome
closes them when the connection that saw them goes. An alert is accepted, a confirm or prompt
cancelled, unless the request says "dialog":"accept" (or "dismiss"); the answer says what it
was ("dialog": {type, message, accepted}).
  {"op":"desk_shot","path":"…"} {"op":"desk_click","x":…,"y":…} {"op":"desk_type","text":"…"}
  {"op":"desk_key","key":"Return"}          the whole screen, with xdotool (native dialogs…)
"""

import base64
import json
import os
import subprocess
import sys
import time
import urllib.request

import websocket  # python3-websocket

DEVTOOLS = "http://127.0.0.1:9222"
STATE = "/tmp/rad-browse-tab"
MAX_TEXT = 3500
MAX_ELEMENTS = 160
TIMEOUT = 15

KEYS = {
    "Enter": (13, "\r"), "Tab": (9, "\t"), "Escape": (27, ""), "Backspace": (8, ""), "Delete": (46, ""),
    "ArrowUp": (38, ""), "ArrowDown": (40, ""), "ArrowLeft": (37, ""), "ArrowRight": (39, ""),
    "PageUp": (33, ""), "PageDown": (34, ""), "Home": (36, ""), "End": (35, ""), "Space": (32, " "),
}


class Fail(Exception):
    pass


def http(path, method="GET"):
    req = urllib.request.Request(DEVTOOLS + path, method=method)
    with urllib.request.urlopen(req, timeout=5) as r:
        body = r.read().decode()
    try:
        return json.loads(body)
    except ValueError:
        return body


def ensure_desktop():
    try:
        http("/json/version")
    except Exception:
        r = subprocess.run(["/opt/rad/rad-desktop"], capture_output=True, text=True, timeout=60)
        if "desktop-ok" not in r.stdout:
            raise Fail("the browser didn't start: " + (r.stdout + r.stderr).strip()[-300:])


def pages():
    return [t for t in http("/json/list") if t.get("type") == "page" and not t.get("url", "").startswith("devtools://")]


def current():
    ps = pages()
    if not ps:
        http("/json/new?about:blank", "PUT")
        ps = pages()
    try:
        want = open(STATE).read().strip()
    except OSError:
        want = ""
    for p in ps:
        if p["id"] == want:
            return p
    set_current(ps[0]["id"])
    return ps[0]


def set_current(tid):
    with open(STATE, "w") as f:
        f.write(tid)


class Tab:
    def __init__(self, target):
        self.target = target
        self.ws = websocket.create_connection(target["webSocketDebuggerUrl"], timeout=TIMEOUT, suppress_origin=True)
        self.n = 0
        # How to answer a pop-up (None: accept alerts, cancel the rest), and the last one seen.
        self.policy = None
        self.dialog = None

    def call(self, method, **params):
        self.n += 1
        self.ws.send(json.dumps({"id": self.n, "method": method, "params": params}))
        while True:
            try:
                m = json.loads(self.ws.recv())
            except websocket.WebSocketTimeoutException:
                raise Fail("the page isn't answering (if a pop-up is open, use dialog)")
            if m.get("method") == "Page.javascriptDialogOpening":
                p = m.get("params", {})
                kind = p.get("type", "alert")
                accept = self.policy if self.policy is not None else kind in ("alert", "beforeunload")
                self.dialog = {"type": kind, "message": p.get("message", "")[:300], "accepted": accept}
                # Its answer has an id of its own, which nothing waits for.
                self.ws.send(json.dumps({"id": 1_000_000 + self.n, "method": "Page.handleJavaScriptDialog", "params": {"accept": accept}}))
            if m.get("id") == self.n:
                if "error" in m:
                    raise Fail(m["error"].get("message", "DevTools error"))
                return m.get("result", {})

    def js(self, expr):
        r = self.call("Runtime.evaluate", expression=expr, returnByValue=True, awaitPromise=True)
        if r.get("exceptionDetails"):
            raise Fail(r["exceptionDetails"].get("exception", {}).get("description", "script error")[:300])
        return r.get("result", {}).get("value")

    def settle(self, ms=600):
        """Waits for the page to finish loading (up to ~8 s), then a moment for scripts."""
        end = time.time() + 8
        time.sleep(0.25)
        while time.time() < end:
            try:
                if self.js("document.readyState") == "complete":
                    break
            except Fail:
                pass
            time.sleep(0.2)
        time.sleep(ms / 1000)

    def close(self):
        try:
            self.ws.close()
        except Exception:
            pass


# Numbers the visible, usable elements and describes the page. Elements keep their number
# while the page lives, so the model can refer to them after scrolling.
SNAPSHOT_JS = r"""
(() => {
  const MAX = %d, MAXTEXT = %d;
  let next = Number(document.documentElement.dataset.radNext || 1);
  const sel = 'a[href],button,input,select,textarea,summary,[role=button],[role=link],[role=checkbox],[role=radio],[role=tab],[role=menuitem],[role=option],[role=switch],[role=combobox],[contenteditable=""],[contenteditable=true],[onclick],[tabindex]:not([tabindex="-1"])';
  const vis = (e) => {
    const r = e.getBoundingClientRect();
    if (r.width < 2 || r.height < 2) return false;
    const s = getComputedStyle(e);
    return s.visibility !== 'hidden' && s.display !== 'none' && Number(s.opacity) > 0.05;
  };
  const label = (e) => {
    const bits = [e.getAttribute('aria-label'), e.labels && e.labels[0] && e.labels[0].innerText, e.getAttribute('placeholder'), e.getAttribute('title'), e.getAttribute('alt'), e.innerText, e.value && e.type !== 'password' ? e.value : '', e.getAttribute('name')];
    for (const b of bits) { const t = (b || '').replace(/\s+/g, ' ').trim(); if (t) return t.slice(0, 80); }
    const img = e.querySelector && e.querySelector('img[alt]');
    return img ? img.alt.slice(0, 80) : '';
  };
  const H = innerHeight;
  const all = [...document.querySelectorAll(sel)].filter(vis);
  // On screen first, then the rest of the page in order.
  const onScreen = (e) => { const r = e.getBoundingClientRect(); return r.bottom > 0 && r.top < H; };
  all.sort((a, b) => (onScreen(b) - onScreen(a)));
  const items = [];
  for (const e of all.slice(0, MAX)) {
    if (!e.dataset.radId) e.dataset.radId = String(next++);
    const tag = e.tagName.toLowerCase();
    const role = e.getAttribute('role') || (tag === 'a' ? 'link' : tag === 'input' ? (e.type || 'text') : tag);
    const it = { id: Number(e.dataset.radId), role, label: label(e) };
    if (tag === 'a' && e.href && !e.href.startsWith('javascript:')) it.href = e.href.slice(0, 120);
    if (tag === 'input' && e.type === 'password') it.password = true;
    if ((tag === 'input' && ['checkbox', 'radio'].includes(e.type)) || e.getAttribute('aria-checked')) it.checked = e.checked || e.getAttribute('aria-checked') === 'true';
    if (tag === 'select') it.options = [...e.options].slice(0, 15).map((o) => o.text.trim().slice(0, 40));
    if (e.disabled) it.disabled = true;
    if (!onScreen(e)) it.offscreen = true;
    items.push(it);
  }
  document.documentElement.dataset.radNext = String(next);
  const text = (document.body ? document.body.innerText : '').replace(/[ \t]+/g, ' ').replace(/\n{3,}/g, '\n\n').trim();
  return {
    url: location.href, title: document.title,
    scroll: { y: Math.round(scrollY), height: Math.round(document.documentElement.scrollHeight), view: H },
    text: text.slice(0, MAXTEXT), more_text: text.length > MAXTEXT,
    elements: items, more_elements: all.length > MAX,
  };
})()
""" % (MAX_ELEMENTS, MAX_TEXT)


def element_box(tab, eid):
    r = tab.js("""(() => {
      const e = document.querySelector('[data-rad-id="%d"]');
      if (!e) return null;
      e.scrollIntoView({block: 'center', inline: 'center'});
      const r = e.getBoundingClientRect();
      const t = (e.getAttribute('aria-label') || e.innerText || e.value || e.getAttribute('placeholder') || '').replace(/\\s+/g, ' ').trim().slice(0, 80);
      return {x: r.left + r.width / 2, y: r.top + r.height / 2, tag: e.tagName.toLowerCase(), type: e.type || '', label: t};
    })()""" % int(eid))
    if not r:
        raise Fail("element %s isn't on the page any more: take a new snapshot" % eid)
    return r


def click_at(tab, x, y):
    for t in ("mouseMoved", "mousePressed", "mouseReleased"):
        tab.call("Input.dispatchMouseEvent", type=t, x=x, y=y, button="left", clickCount=1)


def press(tab, key):
    if key not in KEYS:
        raise Fail("unknown key %s (use one of %s)" % (key, ", ".join(KEYS)))
    code, text = KEYS[key]
    name = " " if key == "Space" else key
    tab.call("Input.dispatchKeyEvent", type="keyDown", key=name, windowsVirtualKeyCode=code, nativeVirtualKeyCode=code, text=text)
    tab.call("Input.dispatchKeyEvent", type="keyUp", key=name, windowsVirtualKeyCode=code, nativeVirtualKeyCode=code)


def replace_text(tab, eid, text):
    b = element_box(tab, eid)
    click_at(tab, b["x"], b["y"])
    tab.js("""(() => { const e = document.querySelector('[data-rad-id="%d"]'); if (!e) return;
      e.focus(); if ('value' in e && e.select) e.select(); else document.execCommand('selectAll'); })()""" % int(eid))
    tab.call("Input.dispatchKeyEvent", type="keyDown", key="Backspace", windowsVirtualKeyCode=8, nativeVirtualKeyCode=8)
    tab.call("Input.dispatchKeyEvent", type="keyUp", key="Backspace", windowsVirtualKeyCode=8, nativeVirtualKeyCode=8)
    if text:
        tab.call("Input.insertText", text=text)
    return b


def hostname(url):
    from urllib.parse import urlparse

    return (urlparse(url).hostname or "").lower()


def on_domain(url, domain):
    h, d = hostname(url), domain.lower().lstrip(".")
    return bool(d) and (h == d or h.endswith("." + d))


def run(req):
    op = req.get("op")
    if op and op.startswith("desk_"):
        return desk(req)
    ensure_desktop()
    target = current()
    before = {p["id"] for p in pages()}
    tab = Tab(target)
    if req.get("dialog") in ("accept", "dismiss"):
        tab.policy = req["dialog"] == "accept"
    try:
        out = page_op(tab, req)
        if tab.dialog is not None:
            out["dialog"] = tab.dialog
    finally:
        tab.close()
    # A click that opened a new tab: carry on in it.
    if op in ("click", "mouse", "key"):
        new = [p for p in pages() if p["id"] not in before]
        if new:
            set_current(new[-1]["id"])
            out["new_tab"] = True
    return out


def page_op(tab, req):
    op = req.get("op")
    # A pop-up still open (one that opened between calls): answered before anything else.
    if op == "dialog":
        tab.call("Page.handleJavaScriptDialog", accept=bool(req.get("accept", True)))
        return {"ok": True}
    tab.call("Page.enable")
    if op == "snapshot":
        return {"ok": True, "page": tab.js(SNAPSHOT_JS)}
    if op == "goto":
        url = req.get("url", "").strip()
        if not url:
            raise Fail("no url")
        if "://" not in url and not url.startswith(("about:", "data:")):
            url = "https://" + url
        r = tab.call("Page.navigate", url=url)
        if r.get("errorText"):
            raise Fail("couldn't open %s: %s" % (url, r["errorText"]))
        tab.settle()
        return {"ok": True}
    if op in ("back", "forward"):
        h = tab.call("Page.getNavigationHistory")
        i = h["currentIndex"] + (-1 if op == "back" else 1)
        if i < 0 or i >= len(h["entries"]):
            raise Fail("there's no page to go %s to" % op)
        tab.call("Page.navigateToHistoryEntry", entryId=h["entries"][i]["id"])
        tab.settle()
        return {"ok": True}
    if op == "reload":
        tab.call("Page.reload")
        tab.settle()
        return {"ok": True}
    if op == "click":
        b = element_box(tab, req.get("id"))
        click_at(tab, b["x"], b["y"])
        tab.settle(500)
        return {"ok": True, "clicked": b["label"]}
    if op == "type":
        replace_text(tab, req.get("id"), str(req.get("text", "")))
        if req.get("enter"):
            press(tab, "Enter")
            tab.settle()
        return {"ok": True}
    if op == "select":
        v = json.dumps(str(req.get("value", "")))
        r = tab.js("""(() => { const e = document.querySelector('[data-rad-id="%d"]'); if (!e || !e.options) return 'not a list';
          const want = %s.toLowerCase(); const o = [...e.options].find(o => o.text.trim().toLowerCase() === want || o.value.toLowerCase() === want)
            || [...e.options].find(o => o.text.toLowerCase().includes(want));
          if (!o) return 'no such option'; e.value = o.value; e.dispatchEvent(new Event('input', {bubbles: true})); e.dispatchEvent(new Event('change', {bubbles: true})); return 'ok'; })()""" % (int(req.get("id")), v))
        if r != "ok":
            raise Fail(r)
        tab.settle(300)
        return {"ok": True}
    if op == "key":
        press(tab, req.get("key", ""))
        tab.settle(400)
        return {"ok": True}
    if op == "scroll":
        sign = -1 if req.get("direction") == "up" else 1
        tab.js("window.scrollBy(0, %d * innerHeight * 0.85)" % sign)
        time.sleep(0.4)
        return {"ok": True}
    if op == "tabs":
        cur = tab.target["id"]
        return {"ok": True, "tabs": [{"index": i, "title": p.get("title", "")[:80], "url": p.get("url", "")[:120], "current": p["id"] == cur} for i, p in enumerate(pages())]}
    if op == "tab":
        ps = pages()
        i = int(req.get("index", 0))
        if not 0 <= i < len(ps):
            raise Fail("there's no tab %d" % i)
        set_current(ps[i]["id"])
        http("/json/activate/" + ps[i]["id"])
        return {"ok": True}
    if op == "close_tab":
        ps = pages()
        if len(ps) > 1:
            http("/json/close/" + tab.target["id"])
            rest = [p for p in ps if p["id"] != tab.target["id"]]
            set_current(rest[0]["id"])
        return {"ok": True}
    if op == "screenshot":
        r = tab.call("Page.captureScreenshot", format="jpeg", quality=int(req.get("quality", 70)))
        path = req.get("path") or "/root/shared/shot.jpg"
        with open(path + ".tmp", "wb") as f:
            f.write(base64.b64decode(r["data"]))
        os.replace(path + ".tmp", path)
        try:
            m = tab.js("({w: innerWidth, h: innerHeight, url: location.href, title: document.title})")
        except Fail:  # a pop-up: the picture still shows it
            m = {"w": None, "h": None, "url": tab.target.get("url", ""), "title": tab.target.get("title", "")}
        return {"ok": True, "width": m["w"], "height": m["h"], "url": m["url"], "title": m["title"]}
    if op == "mouse":
        click_at(tab, float(req.get("x", 0)), float(req.get("y", 0)))
        tab.settle(300)
        return {"ok": True}
    if op == "text":
        tab.call("Input.insertText", text=str(req.get("text", "")))
        return {"ok": True}
    if op == "at":
        r = tab.js("""(() => { let e = document.elementFromPoint(%f, %f);
          const ok = 'a,button,input,select,textarea,summary,[role],[onclick],label';
          while (e && e !== document.body && !e.matches(ok)) e = e.parentElement;
          if (!e || e === document.body) e = document.elementFromPoint(%f, %f);
          if (!e) return null;
          const t = (e.getAttribute('aria-label') || e.innerText || e.getAttribute('placeholder') || e.getAttribute('title') || e.value || '').replace(/\\s+/g, ' ').trim().slice(0, 80);
          return {tag: e.tagName.toLowerCase(), type: e.type || '', role: e.getAttribute('role') || '', label: e.type === 'password' ? '' : t, password: e.type === 'password', href: e.href || ''}; })()"""
                   % ((float(req.get("x", 0)), float(req.get("y", 0))) * 2))
        url = tab.js("location.href")
        return {"ok": True, "element": r, "url": url}
    if op == "fill_secret":
        path = req.get("file", "")
        if not path.startswith("/dev/shm/rad-secret-"):
            raise Fail("bad secret file")
        try:
            secret = open(path).read()
        finally:
            try:
                os.remove(path)
            except OSError:
                pass
        url = tab.js("location.href")
        if not on_domain(url, req.get("domain", "")):
            raise Fail("refused: this page is on %s, not %s" % (hostname(url), req.get("domain")))
        replace_text(tab, req.get("id"), secret)
        secret = None
        return {"ok": True}
    raise Fail("unknown operation %s" % op)


def desk(req):
    env = dict(os.environ, DISPLAY=":0")
    subprocess.run(["/opt/rad/rad-desktop"], capture_output=True, timeout=60)
    op = req["op"]

    def x(*a):
        r = subprocess.run(["xdotool", *a], env=env, capture_output=True, text=True, timeout=20)
        if r.returncode:
            raise Fail(r.stderr.strip() or "xdotool failed")

    if op == "desk_shot":
        path = req.get("path") or "/root/shared/desk.jpg"
        r = subprocess.run(["scrot", "-o", "-q", "70", path + ".tmp.jpg"], env=env, capture_output=True, text=True, timeout=20)
        if r.returncode:
            raise Fail(r.stderr.strip() or "scrot failed")
        os.replace(path + ".tmp.jpg", path)
        return {"ok": True, "width": 1280, "height": 800}
    if op == "desk_click":
        x("mousemove", str(int(req.get("x", 0))), str(int(req.get("y", 0))), "click", "1")
        return {"ok": True}
    if op == "desk_type":
        x("type", "--delay", "20", "--", str(req.get("text", "")))
        return {"ok": True}
    if op == "desk_key":
        x("key", "--", str(req.get("key", "Return")))
        return {"ok": True}
    raise Fail("unknown operation %s" % op)


def main():
    req = {}
    try:
        req = json.loads(base64.b64decode(sys.argv[1]).decode())
        out = run(req)
    except Fail as e:
        out = {"ok": False, "error": str(e)}
    except Exception as e:  # anything unexpected still answers in JSON
        out = {"ok": False, "error": "%s: %s" % (type(e).__name__, e)}
    finally:
        # A secret RAD wrote for this call never outlives it.
        f = req.get("file", "") if isinstance(req, dict) else ""
        if f.startswith("/dev/shm/rad-secret-"):
            try:
                os.remove(f)
            except OSError:
                pass
    text = json.dumps(out, ensure_ascii=False)
    # RAD keeps only the end of long output: a big answer goes through the exchange folder.
    if len(text) > 8000:
        name = "out-%d-%d.json" % (os.getpid(), int(time.time() * 1000))
        with open("/root/shared/" + name, "w") as f:
            f.write(text)
        text = json.dumps({"file": name})
    print(text)


if __name__ == "__main__":
    main()
