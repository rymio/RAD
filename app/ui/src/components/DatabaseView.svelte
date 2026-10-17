<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { ask } from '@tauri-apps/plugin-dialog';
  import { api, rad } from '../lib/api';
  import { store } from '../lib/state.svelte';
  import { formatBytes, timeAgo } from '../lib/util';
  import type { DbColumn, DbInfo, DbKey, DbResult, DbRowsPage, DbTable, DbValue } from '../lib/types';
  import Modal from './Modal.svelte';
  import Menu from './Menu.svelte';
  import {
    Database, Table2, Eye, Plus, Trash2, Pencil, Play, Loader, Search, ChevronLeft, ChevronRight, ArrowUp, ArrowDown, KeyRound,
    TerminalSquare, RefreshCw, Copy, MoreHorizontal, Columns3, Rows3, X, Check, History,
  } from '@lucide/svelte';

  type Mode = 'browse' | 'structure' | 'sql' | 'api' | 'new-table';

  let dbs = $state<DbInfo[]>([]);
  let loaded = $state(false);
  let selDb = $state<string | null>(read('rad-db-selected'));
  let tables = $state<DbTable[]>([]);
  let selTable = $state<string | null>(null);
  let mode = $state<Mode>('browse');

  const db = $derived(dbs.find((d) => d.id === selDb) ?? null);
  const table = $derived(tables.find((t) => t.name === selTable) ?? null);
  const base = $derived(db ? `/v1/db/${encodeURIComponent(db.id)}` : '');
  const tpath = (t: string) => `${base}/tables/${encodeURIComponent(t)}`;

  function read(k: string) {
    try {
      return localStorage.getItem(k);
    } catch {
      return null;
    }
  }
  function write(k: string, v: string) {
    try {
      localStorage.setItem(k, v);
    } catch {
      /* private mode */
    }
  }

  // ------------------------------------------------------------------ databases

  async function loadDbs() {
    try {
      dbs = await api.get<DbInfo[]>('/v1/db');
    } catch (e) {
      store.error(e, 'Couldn’t load the databases');
    }
    loaded = true;
    if (!dbs.some((d) => d.id === selDb)) selDb = dbs[0]?.id ?? null;
    if (selDb) await loadTables();
  }

  async function pickDb(id: string) {
    selDb = id;
    write('rad-db-selected', id);
    selTable = null;
    mode = 'browse';
    await loadTables();
  }

  async function newDb() {
    const name = window.prompt('Name of the new database', dbs.length ? '' : 'main')?.trim();
    if (!name) return;
    try {
      const d = await api.post<DbInfo>('/v1/db', { name });
      await loadDbs();
      await pickDb(d.id);
    } catch (e) {
      store.error(e, 'Couldn’t create the database');
    }
  }

  async function renameDb() {
    if (!db) return;
    const name = window.prompt('Rename database', db.name)?.trim();
    if (!name || name === db.name) return;
    await api.patch(`${base}`, { name }).catch((e) => store.error(e));
    loadDbs();
  }

  async function deleteDb() {
    if (!db) return;
    if (!(await ask(`Delete the database “${db.name}” and everything in it? This can’t be undone (vault backups keep copies).`, { title: 'Delete database', kind: 'warning' }))) return;
    await api.del(base).catch((e) => store.error(e));
    selDb = null;
    tables = [];
    loadDbs();
  }

  // ------------------------------------------------------------------ tables

  async function loadTables() {
    if (!db) return;
    try {
      tables = await api.get<DbTable[]>(`${base}/tables`);
    } catch (e) {
      tables = [];
      store.error(e, 'Couldn’t read the tables');
    }
    if (selTable && !tables.some((t) => t.name === selTable)) selTable = null;
    if (!selTable && tables.length && (mode === 'browse' || mode === 'structure')) pickTable(tables[0].name);
  }

  function pickTable(name: string, m: Mode = mode === 'structure' ? 'structure' : 'browse') {
    selTable = name;
    mode = m;
    offset = 0;
    order = null;
    desc = false;
    q = '';
    picked = new Set();
    if (m === 'browse') loadRows();
  }

  async function renameTable() {
    if (!table) return;
    const to = window.prompt('Rename table', table.name)?.trim();
    if (!to || to === table.name) return;
    try {
      await api.patch(tpath(table.name), { rename_to: to });
      selTable = to;
      await loadTables();
    } catch (e) {
      store.error(e, 'Couldn’t rename the table');
    }
  }

  async function dropTable() {
    if (!table) return;
    if (!(await ask(`Drop the ${table.kind} “${table.name}”${table.rows ? ` and its ${table.rows} rows` : ''}?`, { title: 'Drop table', kind: 'warning' }))) return;
    try {
      await api.del(tpath(table.name));
      selTable = null;
      await loadTables();
    } catch (e) {
      store.error(e, 'Couldn’t drop the table');
    }
  }

  // New table
  const TYPES = ['INTEGER', 'TEXT', 'REAL', 'BLOB', 'NUMERIC', 'BOOLEAN', 'DATETIME', 'JSON'];
  const blankCol = (): DbColumn => ({ name: '', type: 'TEXT', not_null: false, default: null, pk: 0, unique: false });
  let nt = $state<{ name: string; columns: DbColumn[] }>({ name: '', columns: [] });

  function startNewTable() {
    nt = { name: '', columns: [{ name: 'id', type: 'INTEGER', not_null: false, default: null, pk: 1, unique: false }, blankCol()] };
    mode = 'new-table';
    selTable = null;
  }

  async function createTable(e: Event) {
    e.preventDefault();
    const columns = nt.columns.filter((c) => c.name.trim()).map((c, i, all) => ({ ...c, pk: c.pk ? all.filter((x) => x.pk).indexOf(c) + 1 : 0 }));
    try {
      const t = await api.post<DbTable>(`${base}/tables`, { name: nt.name.trim(), columns });
      await loadTables();
      pickTable(t.name, 'browse');
    } catch (err) {
      store.error(err, 'Couldn’t create the table');
    }
  }

  // Columns
  let colOpen = $state(false);
  let col = $state<DbColumn>(blankCol());

  async function addColumn(e: Event) {
    e.preventDefault();
    if (!table) return;
    try {
      await api.post(`${tpath(table.name)}/columns`, col);
      colOpen = false;
      await loadTables();
    } catch (err) {
      store.error(err, 'Couldn’t add the column');
    }
  }

  async function renameColumn(c: DbColumn) {
    if (!table) return;
    const to = window.prompt(`Rename column “${c.name}”`, c.name)?.trim();
    if (!to || to === c.name) return;
    await api.patch(`${tpath(table.name)}/columns/${encodeURIComponent(c.name)}`, { rename_to: to }).catch((e) => store.error(e));
    loadTables();
  }

  async function dropColumn(c: DbColumn) {
    if (!table) return;
    if (!(await ask(`Drop the column “${c.name}” and its data?`, { title: 'Drop column', kind: 'warning' }))) return;
    await api.del(`${tpath(table.name)}/columns/${encodeURIComponent(c.name)}`).catch((e) => store.error(e));
    loadTables();
  }

  // ------------------------------------------------------------------ rows

  const LIMIT = 50;
  let page = $state<DbRowsPage | null>(null);
  let offset = $state(0);
  let order = $state<string | null>(null);
  let desc = $state(false);
  let q = $state('');
  let loadingRows = $state(false);
  let picked = $state(new Set<number>());

  async function loadRows() {
    if (!table) return;
    loadingRows = true;
    const params = new URLSearchParams({ limit: String(LIMIT), offset: String(offset) });
    if (order) params.set('order', order);
    if (desc) params.set('desc', 'true');
    if (q.trim()) params.set('q', q.trim());
    try {
      page = await api.get<DbRowsPage>(`${tpath(table.name)}/rows?${params}`);
    } catch (e) {
      page = null;
      store.error(e, 'Couldn’t read the rows');
    } finally {
      loadingRows = false;
    }
  }

  function sortBy(c: string) {
    if (order === c) desc = !desc;
    else {
      order = c;
      desc = false;
    }
    offset = 0;
    loadRows();
  }

  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  function onSearch() {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      offset = 0;
      loadRows();
    }, 250);
  }

  function togglePick(id: number | null) {
    if (id === null) return;
    const s = new Set(picked);
    s.has(id) ? s.delete(id) : s.add(id);
    picked = s;
  }

  async function deletePicked() {
    if (!table || !picked.size) return;
    if (!(await ask(`Delete ${picked.size} row${picked.size > 1 ? 's' : ''}?`, { title: 'Delete rows', kind: 'warning' }))) return;
    for (const id of picked) {
      await api.del(`${tpath(table.name)}/rows/${id}`).catch((e) => store.error(e));
    }
    picked = new Set();
    await loadRows();
    loadTables();
  }

  // Row editor
  type Field = { isNull: boolean; text: string; blob: boolean };
  let editOpen = $state(false);
  let editRowid = $state<number | null>(null);
  let fields = $state<Record<string, Field>>({});

  function display(v: DbValue): string {
    if (v === null) return 'NULL';
    if (typeof v === 'object') return `blob · ${formatBytes(Math.floor((v.$blob.length * 3) / 4))}`;
    return String(v);
  }

  function openRow(row: { rowid: number | null; values: DbValue[] } | null) {
    if (!table) return;
    editRowid = row?.rowid ?? null;
    const f: Record<string, Field> = {};
    table.columns.forEach((c, i) => {
      const v = row ? row.values[i] : undefined;
      const isBlob = !!v && typeof v === 'object';
      f[c.name] = row
        ? { isNull: v === null, text: v === null || isBlob ? '' : String(v), blob: isBlob }
        : { isNull: !c.not_null && (c.pk > 0 || c.default !== null), text: '', blob: false };
    });
    fields = f;
    editOpen = true;
  }

  function parse(c: DbColumn, text: string): DbValue {
    const t = c.type.toUpperCase();
    const n = Number(text);
    if (text.trim() !== '' && !Number.isNaN(n)) {
      if (/INT/.test(t)) return Number.isInteger(n) ? n : text;
      if (/REAL|FLOA|DOUB|NUM|DEC/.test(t)) return n;
    }
    if (/BOOL/.test(t) && /^(true|false)$/i.test(text.trim())) return /true/i.test(text) ? 1 : 0;
    return text;
  }

  async function saveRow(e: Event) {
    e.preventDefault();
    if (!table) return;
    const values: Record<string, DbValue> = {};
    for (const c of table.columns) {
      const f = fields[c.name];
      if (f.blob) continue; // left as it is
      if (editRowid === null && f.isNull) continue; // let defaults and rowids fill in
      values[c.name] = f.isNull ? null : parse(c, f.text);
    }
    try {
      if (editRowid === null) await api.post(`${tpath(table.name)}/rows`, values);
      else await api.patch(`${tpath(table.name)}/rows/${editRowid}`, values);
      editOpen = false;
      await loadRows();
      loadTables();
    } catch (err) {
      store.error(err, 'Couldn’t save the row');
    }
  }

  // ------------------------------------------------------------------ SQL

  let sql = $state(read('rad-db-sql') ?? 'SELECT name, type FROM sqlite_schema;');
  let results = $state<DbResult[] | null>(null);
  let sqlError = $state<string | null>(null);
  let running = $state(false);
  let history = $state<string[]>(JSON.parse(read('rad-db-history') ?? '[]'));
  let took = $state(0);

  async function runSql() {
    if (!db || !sql.trim() || running) return;
    running = true;
    sqlError = null;
    write('rad-db-sql', sql);
    const t0 = performance.now();
    try {
      results = await api.post<DbResult[]>(`${base}/sql`, { sql });
      history = [sql.trim(), ...history.filter((h) => h !== sql.trim())].slice(0, 30);
      write('rad-db-history', JSON.stringify(history));
      if (results.some((r) => !r.columns.length)) loadTables();
    } catch (e: any) {
      results = null;
      sqlError = e?.message ?? String(e);
    } finally {
      took = Math.round(performance.now() - t0);
      running = false;
    }
  }

  function sqlKey(e: KeyboardEvent) {
    if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
      e.preventDefault();
      runSql();
    }
  }

  // ------------------------------------------------------------------ API keys

  let keys = $state<DbKey[]>([]);
  let apiInfo = $state<{ url: string | null; discovery_file: string | null; local: boolean } | null>(null);
  let keyForm = $state<{ name: string; write: boolean; all: boolean; dbs: string[] }>({ name: '', write: false, all: true, dbs: [] });
  let newKey = $state<string | null>(null);

  async function loadKeys() {
    keys = await api.get<DbKey[]>('/v1/db-keys').catch(() => []);
    apiInfo = await rad.dbApiInfo().catch(() => null);
  }

  async function createKey(e: Event) {
    e.preventDefault();
    try {
      const r = await api.post<{ key: string; info: DbKey }>('/v1/db-keys', { name: keyForm.name, write: keyForm.write, dbs: keyForm.all ? [] : keyForm.dbs });
      newKey = r.key;
      keyForm = { name: '', write: false, all: true, dbs: [] };
      loadKeys();
    } catch (err) {
      store.error(err, 'Couldn’t create the key');
    }
  }

  async function deleteKey(k: DbKey) {
    if (!(await ask(`Delete the API key “${k.name}”? Programs using it lose access at once.`, { title: 'Delete API key', kind: 'warning' }))) return;
    await api.del(`/v1/db-keys/${k.id}`).catch((e) => store.error(e));
    loadKeys();
  }

  function copy(text: string) {
    navigator.clipboard.writeText(text).then(() => store.toast('success', 'Copied', undefined, 1500));
  }

  const example = $derived(
    `URL=$(jq -r .url "${apiInfo?.discovery_file ?? '~/Library/Application Support/com.rymtech.rad/api.json'}")\n` +
      `KEY=radk_…\n\n` +
      `# databases and tables\ncurl -H "Authorization: Bearer $KEY" $URL/v1/db\ncurl -H "Authorization: Bearer $KEY" $URL/v1/db/${db?.name ?? 'main'}/tables\n\n` +
      `# rows: list, add, change, delete\ncurl -H "Authorization: Bearer $KEY" "$URL/v1/db/${db?.name ?? 'main'}/tables/${table?.name ?? 'items'}/rows?limit=20"\n` +
      `curl -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \\\n  -d '{"name": "hello"}' $URL/v1/db/${db?.name ?? 'main'}/tables/${table?.name ?? 'items'}/rows\n` +
      `curl -X PATCH … /rows/1 -d '{"name": "changed"}'      curl -X DELETE … /rows/1\n\n` +
      `# any SQL (read-only keys: SELECT only)\ncurl -H "Authorization: Bearer $KEY" -H "Content-Type: application/json" \\\n  -d '{"sql": "SELECT * FROM ${table?.name ?? 'items'} WHERE id = ?1", "params": [1]}' $URL/v1/db/${db?.name ?? 'main'}/sql`,
  );

  function openMode(m: Mode) {
    mode = m;
    if (m === 'api') loadKeys();
    if (m === 'browse' && table) loadRows();
  }

  onMount(() => {
    loadDbs();
  });
</script>

<div class="dbv">
  <header class="drag">
    <div class="ttl no-drag"><Database size={16} /> Database</div>
    {#if dbs.length}
      <select class="select no-drag pick" value={selDb} onchange={(e) => pickDb((e.currentTarget as HTMLSelectElement).value)}>
        {#each dbs as d (d.id)}<option value={d.id}>{d.name}</option>{/each}
      </select>
      {#if db}<span class="muted sz no-drag">{formatBytes(db.size)} · encrypted</span>{/if}
    {/if}
    <span class="spacer"></span>
    <button class="btn sm no-drag" class:on={mode === 'sql'} disabled={!db} onclick={() => openMode('sql')}><TerminalSquare size={13} /> SQL</button>
    <button class="btn sm no-drag" class:on={mode === 'api'} onclick={() => openMode('api')}><KeyRound size={13} /> API keys</button>
    {#if db}
      <div class="no-drag">
        <Menu items={[{ label: 'Rename database…', icon: Pencil, onclick: renameDb }, null, { label: 'Delete database', icon: Trash2, danger: true, onclick: deleteDb }]}>
          {#snippet trigger(toggle)}<button class="icon-btn" onclick={toggle} aria-label="Database menu"><MoreHorizontal size={16} /></button>{/snippet}
        </Menu>
      </div>
    {/if}
    <button class="icon-btn no-drag" title="New database" onclick={newDb}><Plus size={16} /></button>
  </header>

  {#if !loaded}
    <div class="center"><Loader size={18} class="spin" /></div>
  {:else if !dbs.length && mode !== 'api'}
    <div class="center">
      <div class="intro">
        <div class="big"><Database size={28} /></div>
        <h2>Databases</h2>
        <p>SQL databases kept next to your vault and encrypted with it — run by <strong>Turso</strong>, a SQLite-compatible engine in Rust. Browse and edit tables here, or let other programs use them through the API.</p>
        <button class="btn primary" onclick={newDb}><Plus size={14} /> Create a database</button>
      </div>
    </div>
  {:else}
    <div class="body">
      <aside class="side">
        <div class="sh">
          <span class="section-title">Tables</span>
          <span class="spacer"></span>
          <button class="icon-btn sm" title="Refresh" disabled={!db} onclick={loadTables}><RefreshCw size={12} /></button>
          <button class="icon-btn sm" title="New table" disabled={!db} onclick={startNewTable}><Plus size={13} /></button>
        </div>
        <div class="tl">
          {#each tables as t (t.name)}
            <button class="ti" class:on={selTable === t.name && (mode === 'browse' || mode === 'structure')} onclick={() => pickTable(t.name)}>
              {#if t.kind === 'view'}<Eye size={13} />{:else}<Table2 size={13} />{/if}
              <span class="truncate spacer">{t.name}</span>
              <span class="cnt">{t.rows ?? ''}</span>
            </button>
          {:else}
            {#if db}<button class="ti empty" onclick={startNewTable}><Plus size={13} /> Create a table</button>{/if}
          {/each}
        </div>
      </aside>

      <section class="main">
        {#if mode === 'api'}
          <div class="pane">
            <h3>API for other programs</h3>
            <p class="lead">
              Programs on this computer find RAD’s address in <code>{apiInfo?.discovery_file ?? 'api.json'}</code>
              {#if apiInfo?.url}(now <code>{apiInfo.url}</code>){/if} and sign in with an API key. Keys reach only databases — never your documents or chats — and work while RAD is unlocked.
            </p>
            {#if newKey}
              <div class="newkey">
                <div><strong>New key</strong> — copy it now, it isn’t shown again.</div>
                <div class="kv"><code class="selectable">{newKey}</code><button class="btn sm" onclick={() => copy(newKey!)}><Copy size={13} /> Copy</button><button class="icon-btn sm" onclick={() => (newKey = null)}><X size={13} /></button></div>
              </div>
            {/if}
            <table class="grid keys">
              <thead><tr><th>Name</th><th>Key</th><th>Access</th><th>Databases</th><th>Last used</th><th></th></tr></thead>
              <tbody>
                {#each keys as k (k.id)}
                  <tr>
                    <td>{k.name}</td>
                    <td><code>{k.prefix}…</code></td>
                    <td>{k.write ? 'Read & write' : 'Read only'}</td>
                    <td>{k.dbs.length ? k.dbs.map((id) => dbs.find((d) => d.id === id)?.name ?? id).join(', ') : 'All'}</td>
                    <td class="muted">{k.last_used ? timeAgo(k.last_used) : 'never'}</td>
                    <td><button class="icon-btn sm" title="Delete key" onclick={() => deleteKey(k)}><Trash2 size={13} /></button></td>
                  </tr>
                {:else}
                  <tr><td colspan="6" class="muted">No API keys yet.</td></tr>
                {/each}
              </tbody>
            </table>
            <form class="keyform" onsubmit={createKey}>
              <input class="input" bind:value={keyForm.name} placeholder="Program name, e.g. invoice-sync" required />
              <select class="select" bind:value={keyForm.write}><option value={false}>Read only</option><option value={true}>Read & write</option></select>
              <select class="select" bind:value={keyForm.all}><option value={true}>All databases</option><option value={false}>Chosen databases…</option></select>
              <button class="btn primary" disabled={!keyForm.name.trim() || (!keyForm.all && !keyForm.dbs.length)}><Plus size={13} /> Create key</button>
            </form>
            {#if !keyForm.all}
              <div class="dbpick">
                {#each dbs as d (d.id)}
                  <label><input type="checkbox" checked={keyForm.dbs.includes(d.id)} onchange={(e) => (keyForm.dbs = (e.currentTarget as HTMLInputElement).checked ? [...keyForm.dbs, d.id] : keyForm.dbs.filter((x) => x !== d.id))} /> {d.name}</label>
                {/each}
              </div>
            {/if}
            <div class="exh"><span class="section-title">Examples</span><button class="icon-btn sm" title="Copy" onclick={() => copy(example)}><Copy size={12} /></button></div>
            <pre class="example selectable">{example}</pre>
            <p class="muted small">Values are JSON: numbers, text, null, and blobs as <code>{'{"$blob": "<base64>"}'}</code>. A database can be named by its name or id. Full list of endpoints: <code>crates/rad-server/src/routes/db.rs</code>.</p>
          </div>
        {:else if mode === 'sql' && db}
          <div class="sqlp">
            <div class="sqlbar">
              <span class="section-title">SQL on {db.name}</span>
              <span class="spacer"></span>
              {#if history.length}
                <Menu items={history.slice(0, 15).map((h) => ({ label: h.length > 70 ? h.slice(0, 70) + '…' : h, icon: History, onclick: () => (sql = h) }))}>
                  {#snippet trigger(toggle)}<button class="btn sm ghost" onclick={toggle}><History size={13} /> History</button>{/snippet}
                </Menu>
              {/if}
              <button class="btn sm primary" disabled={running || !sql.trim()} onclick={runSql}>{#if running}<Loader size={13} class="spin" />{:else}<Play size={13} />{/if} Run</button>
            </div>
            <textarea class="sqled selectable" bind:value={sql} spellcheck="false" onkeydown={sqlKey} placeholder="SELECT * FROM … ;  (⌘↵ to run)"></textarea>
            <div class="results">
              {#if sqlError}
                <div class="err">{sqlError}</div>
              {:else if results}
                <div class="muted small">{results.length} statement{results.length > 1 ? 's' : ''} · {took} ms</div>
                {#each results as r, i (i)}
                  {#if r.columns.length}
                    <div class="rhead muted small">{r.rows.length}{r.truncated ? '+' : ''} row{r.rows.length === 1 ? '' : 's'} — <code>{r.sql.length > 90 ? r.sql.slice(0, 90) + '…' : r.sql}</code></div>
                    <div class="gridwrap">
                      <table class="grid">
                        <thead><tr>{#each r.columns as c}<th>{c}</th>{/each}</tr></thead>
                        <tbody>
                          {#each r.rows as row}
                            <tr>{#each row as v}<td class:null={v === null} title={typeof v === 'string' && v.length > 60 ? v : undefined}>{display(v)}</td>{/each}</tr>
                          {/each}
                        </tbody>
                      </table>
                    </div>
                  {:else}
                    <div class="ok small"><Check size={12} /> {r.changes} row{r.changes === 1 ? '' : 's'} changed — <code>{r.sql.length > 90 ? r.sql.slice(0, 90) + '…' : r.sql}</code></div>
                  {/if}
                {/each}
              {:else}
                <div class="muted small">Run one statement or a whole script. Results show here.</div>
              {/if}
            </div>
          </div>
        {:else if mode === 'new-table' && db}
          <form class="pane" onsubmit={createTable}>
            <h3>New table in {db.name}</h3>
            <label class="fl"><span>Table name</span><input class="input" bind:value={nt.name} placeholder="e.g. customers" required /></label>
            <table class="grid cols">
              <thead><tr><th>Column</th><th>Type</th><th title="Primary key">Key</th><th>Not null</th><th>Unique</th><th>Default</th><th></th></tr></thead>
              <tbody>
                {#each nt.columns as c, i}
                  <tr>
                    <td><input class="input" bind:value={c.name} placeholder="name" /></td>
                    <td><select class="select" bind:value={c.type}>{#each TYPES as t}<option>{t}</option>{/each}</select></td>
                    <td><input type="checkbox" checked={c.pk > 0} onchange={(e) => (c.pk = (e.currentTarget as HTMLInputElement).checked ? 1 : 0)} /></td>
                    <td><input type="checkbox" bind:checked={c.not_null} /></td>
                    <td><input type="checkbox" bind:checked={c.unique} /></td>
                    <td><input class="input" value={c.default ?? ''} oninput={(e) => (c.default = (e.currentTarget as HTMLInputElement).value || null)} placeholder="e.g. 0, 'none', CURRENT_TIMESTAMP" /></td>
                    <td><button type="button" class="icon-btn sm" onclick={() => nt.columns.splice(i, 1)}><X size={13} /></button></td>
                  </tr>
                {/each}
              </tbody>
            </table>
            <div class="row">
              <button type="button" class="btn sm" onclick={() => nt.columns.push(blankCol())}><Plus size={13} /> Column</button>
              <span class="spacer"></span>
              <button type="button" class="btn ghost" onclick={() => (mode = 'browse')}>Cancel</button>
              <button class="btn primary" disabled={!nt.name.trim() || !nt.columns.some((c) => c.name.trim())}>Create table</button>
            </div>
            <p class="muted small">An INTEGER primary key numbers rows by itself. Defaults are SQL: numbers as-is, text in single quotes.</p>
          </form>
        {:else if table}
          <div class="tbar">
            <div class="tname">{#if table.kind === 'view'}<Eye size={14} />{:else}<Table2 size={14} />{/if} {table.name}</div>
            <div class="seg">
              <button class:on={mode === 'browse'} onclick={() => openMode('browse')}><Rows3 size={12} /> Browse</button>
              <button class:on={mode === 'structure'} onclick={() => openMode('structure')}><Columns3 size={12} /> Structure</button>
            </div>
            <span class="spacer"></span>
            {#if mode === 'browse'}
              <div class="find"><Search size={13} /><input bind:value={q} oninput={onSearch} placeholder="Find in rows" /></div>
              {#if picked.size}<button class="btn sm danger" onclick={deletePicked}><Trash2 size={13} /> Delete {picked.size}</button>{/if}
              {#if table.kind === 'table'}<button class="btn sm primary" onclick={() => openRow(null)}><Plus size={13} /> Row</button>{/if}
            {:else}
              {#if table.kind === 'table'}<button class="btn sm" onclick={() => ((col = blankCol()), (colOpen = true))}><Plus size={13} /> Column</button>{/if}
              <button class="btn sm" onclick={renameTable}><Pencil size={13} /> Rename</button>
              <button class="btn sm danger" onclick={dropTable}><Trash2 size={13} /> Drop</button>
            {/if}
          </div>

          {#if mode === 'browse'}
            <div class="gridwrap fill">
              {#if page}
                <table class="grid rows">
                  <thead>
                    <tr>
                      <th class="chk"></th>
                      {#each page.columns as c}
                        <th><button class="sort" onclick={() => sortBy(c)}>{c} {#if order === c}{#if desc}<ArrowDown size={11} />{:else}<ArrowUp size={11} />{/if}{/if}</button></th>
                      {/each}
                    </tr>
                  </thead>
                  <tbody>
                    {#each page.rows as r (r.rowid ?? Math.random())}
                      <tr class:sel={r.rowid !== null && picked.has(r.rowid)} ondblclick={() => table?.kind === 'table' && openRow(r)}>
                        <td class="chk">{#if r.rowid !== null}<input type="checkbox" checked={picked.has(r.rowid)} onchange={() => togglePick(r.rowid)} />{/if}</td>
                        {#each r.values as v}<td class:null={v === null} title={typeof v === 'string' && v.length > 60 ? v : undefined}>{display(v)}</td>{/each}
                      </tr>
                    {:else}
                      <tr><td colspan={page.columns.length + 1} class="muted">{q ? 'Nothing matches.' : 'No rows yet.'}</td></tr>
                    {/each}
                  </tbody>
                </table>
              {:else if loadingRows}
                <div class="center"><Loader size={16} class="spin" /></div>
              {/if}
            </div>
            {#if page}
              <div class="pager">
                <span class="muted small">{page.total ? `${page.offset + 1}–${Math.min(page.offset + page.rows.length, page.total)} of ${page.total}` : '0 rows'} · double-click a row to edit</span>
                <span class="spacer"></span>
                {#if loadingRows}<Loader size={13} class="spin" />{/if}
                <button class="icon-btn sm" disabled={offset === 0} onclick={() => ((offset = Math.max(0, offset - LIMIT)), loadRows())}><ChevronLeft size={14} /></button>
                <button class="icon-btn sm" disabled={offset + LIMIT >= page.total} onclick={() => ((offset += LIMIT), loadRows())}><ChevronRight size={14} /></button>
              </div>
            {/if}
          {:else}
            <div class="pane">
              <table class="grid">
                <thead><tr><th>Column</th><th>Type</th><th>Key</th><th>Not null</th><th>Unique</th><th>Default</th><th></th></tr></thead>
                <tbody>
                  {#each table.columns as c (c.name)}
                    <tr>
                      <td><strong>{c.name}</strong></td>
                      <td>{c.type || '—'}</td>
                      <td>{c.pk ? 'Primary' : ''}</td>
                      <td>{c.not_null ? 'yes' : ''}</td>
                      <td>{c.unique ? 'yes' : ''}</td>
                      <td><code>{c.default ?? ''}</code></td>
                      <td class="acts">
                        {#if table.kind === 'table'}
                          <button class="icon-btn sm" title="Rename column" onclick={() => renameColumn(c)}><Pencil size={12} /></button>
                          <button class="icon-btn sm" title="Drop column" disabled={c.pk > 0} onclick={() => dropColumn(c)}><Trash2 size={12} /></button>
                        {/if}
                      </td>
                    </tr>
                  {/each}
                </tbody>
              </table>
              {#if table.indexes.length}
                <div class="section-title gap">Indexes</div>
                {#each table.indexes as ix}<div class="small"><code>{ix.name}</code> on {ix.columns.join(', ')}{ix.unique ? ' (unique)' : ''}</div>{/each}
              {/if}
              <div class="section-title gap">SQL</div>
              <pre class="example selectable">{table.sql}</pre>
            </div>
          {/if}
        {:else}
          <div class="center muted">{db ? (tables.length ? 'Choose a table on the left.' : 'This database has no tables yet.') : 'Choose a database.'}</div>
        {/if}
      </section>
    </div>
  {/if}
</div>

<Modal bind:open={editOpen} title={editRowid === null ? `New row in ${table?.name ?? ''}` : `Row ${editRowid}`} width={560}>
  <form class="rowform" onsubmit={saveRow} id="rowform">
    {#each table?.columns ?? [] as c (c.name)}
      {@const f = fields[c.name]}
      {#if f}
        <label class="rf">
          <span class="rl"><strong>{c.name}</strong> <span class="muted">{c.type}{c.pk ? ' · key' : ''}{c.not_null ? ' · required' : ''}</span></span>
          {#if f.blob}
            <span class="muted small">Binary data — kept as it is (change it with SQL or the API).</span>
          {:else}
            <div class="rv">
              {#if /TEXT|CHAR|CLOB|JSON/i.test(c.type) && f.text.length > 60}
                <textarea class="input" rows="3" bind:value={f.text} disabled={f.isNull}></textarea>
              {:else}
                <input class="input" bind:value={f.text} disabled={f.isNull} placeholder={f.isNull ? (editRowid === null && (c.pk || c.default !== null) ? 'automatic' : 'NULL') : ''} />
              {/if}
              {#if !c.not_null}<label class="nul"><input type="checkbox" bind:checked={f.isNull} /> {editRowid === null && (c.pk || c.default !== null) ? 'auto' : 'null'}</label>{/if}
            </div>
          {/if}
        </label>
      {/if}
    {/each}
  </form>
  {#snippet footer()}
    {#if editRowid !== null}
      <button class="btn danger" onclick={async () => { picked = new Set([editRowid!]); editOpen = false; await tick(); deletePicked(); }}><Trash2 size={13} /> Delete</button>
    {/if}
    <span class="spacer"></span>
    <button class="btn ghost" onclick={() => (editOpen = false)}>Cancel</button>
    <button class="btn primary" type="submit" form="rowform">{editRowid === null ? 'Add row' : 'Save'}</button>
  {/snippet}
</Modal>

<Modal bind:open={colOpen} title={`Add a column to ${table?.name ?? ''}`} width={460}>
  <form class="rowform" onsubmit={addColumn} id="colform">
    <label class="fl"><span>Name</span><input class="input" bind:value={col.name} required /></label>
    <label class="fl"><span>Type</span><select class="select" bind:value={col.type}>{#each TYPES as t}<option>{t}</option>{/each}</select></label>
    <label class="fl"><span>Default</span><input class="input" value={col.default ?? ''} oninput={(e) => (col.default = (e.currentTarget as HTMLInputElement).value || null)} placeholder="needed if not null, e.g. 0 or ''" /></label>
    <label class="nul"><input type="checkbox" bind:checked={col.not_null} /> Not null</label>
  </form>
  {#snippet footer()}
    <span class="spacer"></span>
    <button class="btn ghost" onclick={() => (colOpen = false)}>Cancel</button>
    <button class="btn primary" type="submit" form="colform" disabled={!col.name.trim()}>Add column</button>
  {/snippet}
</Modal>

<style>
  .dbv {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  header {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    height: var(--titlebar-h);
    padding: 0 12px 0 calc(14px + var(--lead));
    border-bottom: 1px solid var(--border);
  }
  .ttl {
    display: flex;
    align-items: center;
    gap: 7px;
    font-weight: 650;
    font-size: 14px;
  }
  .ttl :global(svg) {
    color: var(--accent);
  }
  .pick {
    max-width: 200px;
    height: 28px;
    padding-top: 0;
    padding-bottom: 0;
    font-size: 12.5px;
  }
  .sz {
    font-size: 12px;
  }
  .btn.on {
    border-color: var(--accent);
    color: var(--accent);
  }
  .center {
    flex: 1;
    display: grid;
    place-items: center;
    padding: 24px;
  }
  .intro {
    max-width: 460px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    text-align: center;
  }
  .intro p {
    margin: 0 0 6px;
    color: var(--text-2);
    font-size: 13px;
    line-height: 1.5;
  }
  .intro h2 {
    margin: 0;
    font-size: 19px;
  }
  .big {
    display: grid;
    place-items: center;
    width: 56px;
    height: 56px;
    border-radius: 16px;
    background: var(--accent-soft, var(--surface-2));
    color: var(--accent);
  }
  .body {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .side {
    flex: none;
    width: 210px;
    display: flex;
    flex-direction: column;
    border-right: 1px solid var(--border);
    min-height: 0;
  }
  .sh {
    display: flex;
    align-items: center;
    gap: 4px;
    height: 36px;
    padding: 0 8px 0 12px;
  }
  .tl {
    flex: 1;
    overflow: auto;
    padding: 0 6px 8px;
  }
  .ti {
    display: flex;
    align-items: center;
    gap: 7px;
    width: 100%;
    height: 28px;
    padding: 0 8px;
    border: none;
    border-radius: var(--r-sm);
    background: transparent;
    color: var(--text-2);
    font-size: 13px;
    text-align: left;
  }
  .ti:hover {
    background: var(--surface-hover);
  }
  .ti.on {
    background: var(--surface-active, var(--surface-hover));
    color: var(--text);
    font-weight: 550;
  }
  .ti :global(svg) {
    flex: none;
    color: var(--text-3);
  }
  .ti.empty {
    color: var(--accent);
  }
  .cnt {
    font-size: 11px;
    color: var(--text-3);
  }
  .main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .tbar,
  .sqlbar {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    height: 44px;
    padding: 0 12px;
    border-bottom: 1px solid var(--border);
  }
  .tname {
    display: flex;
    align-items: center;
    gap: 6px;
    font-weight: 650;
    font-size: 13.5px;
  }
  .seg {
    display: flex;
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    overflow: hidden;
  }
  .seg button {
    display: flex;
    align-items: center;
    gap: 5px;
    height: 26px;
    padding: 0 10px;
    border: none;
    background: transparent;
    color: var(--text-2);
    font-size: 12px;
  }
  .seg button.on {
    background: var(--accent);
    color: white;
  }
  .find {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 10px;
    border: 1px solid var(--border);
    border-radius: 999px;
    color: var(--text-3);
  }
  .find input {
    border: none;
    outline: none;
    background: transparent;
    color: var(--text);
    font-size: 12.5px;
    width: 160px;
  }
  .gridwrap {
    overflow: auto;
    max-width: 100%;
  }
  .gridwrap.fill {
    flex: 1;
    min-height: 0;
  }
  .grid {
    border-collapse: collapse;
    font-size: 12.5px;
    width: max-content;
    min-width: 100%;
  }
  .grid th,
  .grid td {
    padding: 6px 10px;
    border-bottom: 1px solid var(--border);
    text-align: left;
    white-space: nowrap;
    max-width: 340px;
    overflow: hidden;
    text-overflow: ellipsis;
    vertical-align: middle;
  }
  .grid th {
    position: sticky;
    top: 0;
    z-index: 1;
    background: var(--surface-2);
    font-weight: 600;
    color: var(--text-2);
  }
  .grid.rows tbody tr:hover {
    background: var(--surface-hover);
  }
  .grid tr.sel {
    background: var(--accent-soft, var(--surface-hover));
  }
  .grid td.null {
    color: var(--text-4, var(--text-3));
    font-style: italic;
  }
  .grid .chk {
    width: 28px;
    padding-right: 0;
  }
  .sort {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    border: none;
    background: none;
    padding: 0;
    color: inherit;
    font: inherit;
  }
  .pager {
    flex: none;
    display: flex;
    align-items: center;
    gap: 6px;
    height: 36px;
    padding: 0 12px;
    border-top: 1px solid var(--border);
  }
  .pane {
    flex: 1;
    overflow: auto;
    padding: 16px 18px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .pane h3 {
    margin: 0;
    font-size: 15px;
  }
  .lead {
    margin: 0;
    font-size: 13px;
    color: var(--text-2);
    line-height: 1.5;
  }
  code {
    font-family: var(--mono);
    font-size: 11.5px;
    padding: 1px 4px;
    border-radius: var(--r-xs);
    background: var(--code-bg);
  }
  .small {
    font-size: 12px;
  }
  .gap {
    margin-top: 10px;
  }
  .example {
    margin: 0;
    padding: 10px 12px;
    border-radius: var(--r-sm);
    background: #14141b;
    color: #e6e6ef;
    font-family: var(--mono);
    font-size: 11.5px;
    line-height: 1.5;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .exh {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 6px;
  }
  .newkey {
    padding: 10px 12px;
    border: 1px solid var(--success);
    border-radius: var(--r-sm);
    background: var(--success-soft);
    font-size: 13px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .kv {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .kv code {
    flex: 1;
    min-width: 0;
    overflow-wrap: anywhere;
    font-size: 12px;
  }
  .keyform {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  .keyform .input {
    flex: 1;
    min-width: 200px;
  }
  .dbpick {
    display: flex;
    flex-wrap: wrap;
    gap: 12px;
    font-size: 13px;
  }
  .dbpick label,
  .nul {
    display: flex;
    align-items: center;
    gap: 5px;
    font-size: 12.5px;
    color: var(--text-2);
  }
  .sqlp {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .sqled {
    flex: none;
    height: 170px;
    resize: vertical;
    border: none;
    border-bottom: 1px solid var(--border);
    outline: none;
    padding: 12px 14px;
    background: var(--surface);
    color: var(--text);
    font-family: var(--mono);
    font-size: 12.5px;
    line-height: 1.55;
  }
  .results {
    flex: 1;
    overflow: auto;
    padding: 10px 14px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .rhead {
    margin-top: 6px;
  }
  .err {
    padding: 10px 12px;
    border-radius: var(--r-sm);
    background: var(--danger-soft);
    color: var(--danger);
    font-family: var(--mono);
    font-size: 12px;
    white-space: pre-wrap;
  }
  .ok {
    display: flex;
    align-items: center;
    gap: 5px;
    color: var(--success);
  }
  .fl {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12.5px;
    color: var(--text-2);
    max-width: 360px;
  }
  .grid.cols td {
    overflow: visible;
  }
  .grid.cols .input,
  .grid.cols .select {
    height: 30px;
    font-size: 12.5px;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .acts {
    text-align: right;
  }
  .rowform {
    display: flex;
    flex-direction: column;
    gap: 10px;
    max-height: 60vh;
    overflow: auto;
  }
  .rf {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .rl {
    font-size: 12.5px;
  }
  .rv {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .rv .input {
    flex: 1;
  }
</style>
