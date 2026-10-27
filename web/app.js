(() => {
  console.info("wryme connector v8");
  const DB = "wryme";
  const STORE = "config";
  const KEY = "dir";
  const SHOPS = "shops.toml";
  const STATIONS = "stations.toml";
  const PREFS = "prefs.json";
  const WRITE = { mode: "readwrite" };

  let dir = null;
  let granted = false;
  let armed = false;
  let wrap = null;
  let button = null;
  let status = null;
  let hint = null;

  function openDb() {
    return new Promise((resolve, reject) => {
      const req = indexedDB.open(DB, 1);
      req.onupgradeneeded = () => req.result.createObjectStore(STORE);
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => reject(req.error);
    });
  }

  async function dbGet(key) {
    const db = await openDb();
    return new Promise((resolve, reject) => {
      const r = db.transaction(STORE, "readonly").objectStore(STORE).get(key);
      r.onsuccess = () => resolve(r.result ?? null);
      r.onerror = () => reject(r.error);
    });
  }

  async function dbSet(key, value) {
    const db = await openDb();
    return new Promise((resolve, reject) => {
      const r = db.transaction(STORE, "readwrite").objectStore(STORE).put(value, key);
      r.onsuccess = () => resolve();
      r.onerror = () => reject(r.error);
    });
  }

  async function opfs() {
    try {
      return await navigator.storage.getDirectory();
    } catch {
      return null;
    }
  }

  async function readFile(root, name) {
    if (!root) return null;
    try {
      const fh = await root.getFileHandle(name);
      return await (await fh.getFile()).text();
    } catch {
      return null;
    }
  }

  async function writeFile(root, name, content) {
    if (!root) return false;
    try {
      const fh = await root.getFileHandle(name, { create: true });
      const w = await fh.createWritable();
      await w.write(content);
      await w.close();
      return true;
    } catch {
      return false;
    }
  }

  async function queryPermission(handle) {
    try {
      return await handle.queryPermission(WRITE);
    } catch {
      return "denied";
    }
  }

  async function requestPermission(handle) {
    try {
      return await handle.requestPermission(WRITE);
    } catch {
      return "denied";
    }
  }

  function push(shops, stations, connected) {
    if (!window.wrymeSetConfig) return;
    window.wrymeSetConfig(shops, stations, connected);
  }

  function pushPrefs(prefs) {
    if (!window.wrymeSetPrefs) return;
    window.wrymeSetPrefs(prefs);
  }

  async function showCached() {
    const root = await opfs();
    const shops = await readFile(root, SHOPS);
    const stations = await readFile(root, STATIONS);
    const prefs = await readFile(root, PREFS);
    if (shops === null && stations === null) return;
    push(shops, stations, false);
    pushPrefs(prefs);
  }

  async function sync() {
    if (!dir) return;
    const shops = await readFile(dir, SHOPS);
    const stations = await readFile(dir, STATIONS);
    const prefs = await readFile(dir, PREFS);
    const root = await opfs();
    if (shops !== null) await writeFile(root, SHOPS, shops);
    if (stations !== null) await writeFile(root, STATIONS, stations);
    if (prefs !== null) await writeFile(root, PREFS, prefs);
    granted = true;
    push(shops, stations, true);
    pushPrefs(prefs);
  }

  async function grant() {
    if (!dir) return false;
    if ((await queryPermission(dir)) === "granted") return true;
    return (await requestPermission(dir)) === "granted";
  }

  async function hasConfig(handle) {
    for (const name of [SHOPS, STATIONS]) {
      try {
        await handle.getFileHandle(name);
        return true;
      } catch {}
    }
    return false;
  }

  async function resolve(handle) {
    if (await hasConfig(handle)) return handle;
    try {
      return await handle.getDirectoryHandle("wryme");
    } catch {}
    try {
      const config = await handle.getDirectoryHandle(".config");
      try {
        return await config.getDirectoryHandle("wryme");
      } catch {
        return await config.getDirectoryHandle("wryme", { create: true });
      }
    } catch {}
    if (handle.name === "wryme") return handle;
    if (handle.name === ".config") {
      try {
        return await handle.getDirectoryHandle("wryme", { create: true });
      } catch {}
    }
    return null;
  }

  async function pick() {
    if (!window.showDirectoryPicker) {
      setStatus("this browser can't connect folders");
      return;
    }
    const opts = { id: "wryme-config", mode: "readwrite" };
    let chosen;
    try {
      chosen = await window.showDirectoryPicker(opts);
    } catch {
      setStatus("no folder picked. in the dialog press ⌘⇧. to reveal dot folders, open .config, then choose wryme");
      return;
    }
    const resolved = await resolve(chosen);
    if (!resolved) {
      setStatus("pick the wryme folder inside .config (⌘⇧. reveals it), or drag it in from Finder");
      return;
    }
    dir = resolved;
    try {
      await dbSet(KEY, dir);
    } catch {}
    await sync();
    render();
    focusTerm();
  }

  window.wrymeWriteFile = async (name, content) => {
    if (!dir || !granted) return false;
    const ok = await writeFile(dir, name, content);
    if (ok) await writeFile(await opfs(), name, content);
    return ok;
  };

  const DOORWAY = "ws://127.0.0.1:9630/";
  let meshId = null;
  let sock = null;
  let seq = 0;
  const pending = new Map();
  let retryMs = 1000;

  window.wrymeSetWindowId = (id) => {
    meshId = String(id);
    connect();
  };

  window.wrymeMeshSend = (json) => new Promise((resolve) => {
    let note;
    try {
      note = JSON.parse(json);
    } catch {
      resolve(JSON.stringify({ ok: false, error: "bad note" }));
      return;
    }
    if (!sock || sock.readyState !== WebSocket.OPEN) {
      resolve(JSON.stringify({ ok: false, error: "no doorway, no live windows" }));
      return;
    }
    const req = ++seq;
    note.req = req;
    const timer = setTimeout(() => {
      pending.delete(req);
      resolve(JSON.stringify({ ok: false, error: "doorway timed out" }));
    }, 10000);
    pending.set(req, (answer) => {
      clearTimeout(timer);
      resolve(answer);
    });
    sock.send(JSON.stringify(note));
  });

  function connect() {
    if (sock && (sock.readyState === WebSocket.OPEN || sock.readyState === WebSocket.CONNECTING)) return;
    let ws;
    try {
      ws = new WebSocket(DOORWAY);
    } catch {
      schedule();
      return;
    }
    ws.onopen = () => {
      retryMs = 1000;
      if (meshId) ws.send(JSON.stringify({ claim: meshId }));
    };
    ws.onmessage = (ev) => {
      let v;
      try {
        v = JSON.parse(ev.data);
      } catch {
        return;
      }
      if (v.roster && window.wrymeSetRoster) {
        window.wrymeSetRoster(JSON.stringify(v.roster));
      }
      if (v.req && pending.has(v.req)) {
        pending.get(v.req)(ev.data);
        pending.delete(v.req);
        return;
      }
      if (v.from && v.text && window.wrymeGuestNote) {
        window.wrymeGuestNote(String(v.from), String(v.id || ""), String(v.text), String(v.reply_to || ""));
      }
    };
    const down = () => {
      if (sock === ws) sock = null;
      if (window.wrymeSetRoster) window.wrymeSetRoster("[]");
      schedule();
    };
    ws.onclose = down;
    sock = ws;
  }

  function schedule() {
    setTimeout(() => {
      retryMs = Math.min(retryMs * 2, 15000);
      connect();
    }, retryMs);
  }

  window.wrymeReady = async () => {
    if (window.__wrymeReady) return;
    window.__wrymeReady = true;
    await showCached();
    try {
      const saved = await dbGet(KEY);
      if (saved) {
        dir = saved;
        if ((await queryPermission(saved)) === "granted") {
          await sync();
        }
      }
    } catch (e) {
      console.error("wryme: restore failed", e);
    }
    render();
  };

  async function onClick() {
    if (dir && (await grant())) {
      await sync();
      render();
      focusTerm();
      return;
    }
    if (!dir && !armed) {
      armed = true;
      setStatus("⌘⇧. reveals dot folders. open .config, pick wryme, Select. or drag the folder in from Finder");
      button.textContent = "Open the panel";
      return;
    }
    armed = false;
    button.textContent = "Connect machine";
    await pick();
  }

  async function copyPath() {
    try {
      await navigator.clipboard.writeText("~/.config/wryme");
      setStatus("path copied. ⌘⇧. is the reliable way to reveal dot folders");
    } catch {
      setStatus("select + copy: ~/.config/wryme");
    }
  }

  function render() {
    if (!button) return;
    if (!dir) {
      wrap.hidden = false;
      button.textContent = "Connect machine";
      status.textContent = "";
      hint.hidden = false;
    } else if (granted) {
      wrap.hidden = true;
    } else {
      wrap.hidden = false;
      button.textContent = "Reconnect machine";
      status.textContent = dir.name + " (cached, reconnect to save)";
      hint.hidden = true;
    }
  }

  function setStatus(text) {
    if (status) status.textContent = text;
  }

  function focusTerm() {
    const term = document.getElementById("wryme-term");
    if (term) term.focus();
  }

  function build() {
    wrap = document.createElement("div");
    wrap.id = "wryme-config";
    status = document.createElement("span");
    status.id = "wryme-config-status";
    const row = document.createElement("div");
    row.id = "wryme-config-row";
    hint = document.createElement("button");
    hint.id = "wryme-config-hint";
    hint.type = "button";
    hint.tabIndex = -1;
    hint.textContent = "copy ~/.config/wryme";
    hint.title = "copy the path, then open the panel";
    hint.addEventListener("click", copyPath);
    button = document.createElement("button");
    button.id = "wryme-config-btn";
    button.type = "button";
    button.tabIndex = -1;
    button.textContent = "Connect machine";
    button.addEventListener("click", onClick);
    row.append(hint, button);
    wrap.append(status, row);
    document.body.append(wrap);
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", build);
  } else {
    build();
  }
})();
