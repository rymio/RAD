use anyhow::{Result, bail};
use rustpython_vm::builtins::{PyBaseExceptionRef, PyDict, PyDictRef, PyList};
use rustpython_vm::function::OptionalArg;
use rustpython_vm::scope::Scope;
use rustpython_vm::{AsObject, Interpreter, PyObjectRef, PyResult, VirtualMachine, compiler};
use std::collections::HashMap;
use std::fs;
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};

/// RymScript's `.win` contract: a script defines top-level state plus a
/// `def draw(events):` function returning a list of widget dicts. The
/// runtime calls `draw` once per rendered frame, feeding back an `events`
/// dict describing what happened to each named widget last frame (a
/// button click, a text field's current value, a checkbox's current
/// state) -- the same "declare this frame's UI, react to what happened
/// last frame" contract every immediate-mode GUI uses, just expressed as
/// plain data instead of live callback references. That avoids needing
/// to hand a live `&mut egui::Ui` into the interpreter at all, which is
/// what makes this safe to build without unsafe code.
pub enum EventValue {
    Clicked,
    Text(String),
    Bool(bool),
}

#[derive(Debug)]
pub enum Widget {
    Heading(String),
    Label(String),
    Separator,
    /// `color`/`enabled` are optional in the dict a script returns
    /// (`None`/`true` respectively when omitted), so every script
    /// written against the original contract keeps rendering exactly
    /// as before.
    Button { id: String, text: String, color: Option<[u8; 3]>, enabled: bool },
    TextInput { id: String, value: String },
    Checkbox { id: String, text: String, value: bool },
    /// Lays children out left-to-right instead of the default top-to-
    /// bottom stack. No `id` of its own -- `ui.horizontal` doesn't need
    /// one, it's not stateful the way `Grid` is.
    Row(Vec<Widget>),
    /// An explicit vertical stack, mostly useful nested inside a `Row`
    /// (a `Row` of `Column`s is how you'd lay out side-by-side panes).
    Column(Vec<Widget>),
    /// A fixed-column-count grid, wrapping to a new row every `columns`
    /// children -- what makes a calculator's keypad or a Minesweeper
    /// board's cells expressible at all. `id` is required (unlike `Row`/
    /// `Column`) because `egui::Grid` is itself a stateful widget that
    /// needs a stable identity across frames.
    Grid { id: String, columns: usize, children: Vec<Widget> },
}

/// The curated stdlib allowlist every RymScript execution point in this
/// project (`.win` via `WinAppRunner` here, `.run` previews/interactive
/// runs in `rymcode.rs`/`terminal.rs`) builds its interpreter with,
/// instead of `Interpreter::without_stdlib`. Each of these is a native
/// Rust module in `rustpython-stdlib` with no path to the host
/// filesystem, network, or process table -- deliberately excluded:
/// `os`, `socket`, `subprocess`, `mmap`, `select`, `ctypes`, and every
/// other module gated behind that crate's own `host_env` feature (which
/// this workspace disables entirely, so those modules aren't even
/// compiled in, not just unregistered).
///
/// Several modules that exist as native Rust code turned out to be
/// unusable here even so, discovered by writing a probe test rather
/// than assumed: `json`, `bisect`, `statistics`, `bz2`, and `lzma`
/// register under CPython's internal accelerator name (`_json`, ...)
/// because in upstream CPython/RustPython the friendly public API is a
/// pure-Python wrapper around that accelerator -- and unlike `struct`
/// (whose `_struct` *is* the complete public API under a different
/// name), these accelerators only expose low-level primitives, not the
/// functions a script would actually call (`_json` has no `.loads`,
/// `_random` has no module-level `.random()`, etc). `re` and `array`
/// are absent for the same underlying reason (`array` transitively
/// needs `collections.abc`, also pure-Python). Building the real
/// pure-Python wrapper support (vendoring `.py` stdlib source, using
/// RustPython's frozen-module build step) is a materially bigger,
/// separate undertaking, not done here.
///
/// `random` and hashing are included anyway, deliberately under a
/// *different* calling convention than real Python, documented in
/// RYMSCRIPT_SDK.md: `_random`'s accelerator is a `Random` class (so
/// `random.Random().random()`, not `random.random()`), and each hash
/// algorithm is its own small module (`sha256.sha256(data)`, not
/// `hashlib.sha256(data)`) rather than one unified `hashlib` dispatcher.
/// `json` gets real support anyway too, but not via this mechanism at
/// all -- see `json_loads`/`json_dumps` in `install_helpers` below,
/// hand-implemented against `serde_json` (already a dependency
/// everywhere in this workspace) rather than RustPython's incomplete
/// native module.
const ALLOWED_STDLIB_MODULES: &[&str] = &[
    "math",
    "cmath",
    "unicodedata",
    "zlib",
    "binascii",
    "_csv",
    "_struct",
    "_random",
    // `_hashlib` is deliberately never aliased to a friendly name (its API
    // shape doesn't match any of the small per-algorithm modules below),
    // but it still has to be registered: `_md5`/`_sha1`/etc all lazily
    // `vm.import("_hashlib", 0)` from their own `module_exec` to reach the
    // shared `PyHasher` type those algorithm modules build their hasher
    // objects from. Leaving `_hashlib` out doesn't make it unimportable --
    // that `vm.import` call swallows its own error -- it makes `PyHasher`'s
    // static type never get created, which panics deep inside RustPython
    // (`class.rs`, "static type has not been initialized") the first time
    // any hash function actually runs. Found by running the hash functions,
    // not by reading the source.
    "_hashlib",
    "_md5",
    "_sha1",
    "_sha256",
    "_sha3",
    "_sha512",
    "_blake2",
];

const MODULE_ALIASES: &[(&str, &str)] = &[
    ("struct", "_struct"),
    ("random", "_random"),
    ("csv", "_csv"),
    ("md5", "_md5"),
    ("sha1", "_sha1"),
    ("sha256", "_sha256"),
    ("sha3", "_sha3"),
    ("sha512", "_sha512"),
    ("blake2", "_blake2"),
];

/// Builds a RymScript interpreter with exactly `ALLOWED_STDLIB_MODULES`
/// registered -- everything else `rustpython-stdlib` ships (including
/// every host-touching module) is left out, whether or not it happened
/// to get compiled in -- plus `MODULE_ALIASES` applied so scripts can
/// `import json` rather than needing to know it's really `_json`.
pub(crate) fn build_interpreter() -> Interpreter {
    let builder = Interpreter::builder(Default::default());
    let ctx = builder.ctx.clone();
    let modules: Vec<&'static rustpython_vm::builtins::PyModuleDef> = rustpython_stdlib::stdlib_module_defs(&ctx)
        .into_iter()
        .filter(|def| ALLOWED_STDLIB_MODULES.contains(&def.name.as_str()))
        .collect();
    builder.add_native_modules(&modules).build()
}

/// Registers each `MODULE_ALIASES` friendly name in `sys.modules`, the
/// same mechanism CPython's own frozen/built-in modules use. This has
/// to run as *compiled Python source* executed through the normal
/// `run_code_obj` path, not a bare Rust-side `vm.import()` call --
/// tried that first, and it hit a native panic ("static type has not
/// been initialized") because these multi-phase-init native modules
/// only complete their exec phase correctly when driven through the
/// VM's real import-statement bytecode path, not called cold from Rust.
fn install_module_aliases(scope: &Scope, vm: &VirtualMachine) -> PyResult<()> {
    let mut source = String::from("import sys as _rym_alias_sys\n");
    for (_, internal) in MODULE_ALIASES {
        source.push_str(&format!("import {internal}\n"));
    }
    // `_csv` has the same "accelerator with no pure-Python wrapper" gap as
    // `_json`/`_random`, just narrower: everything works except the
    // default "excel" dialect, which real CPython's `csv.py` registers at
    // import time via `_csv.register_dialect(...)` -- a call this crate
    // never makes on its own. Without it, `csv.writer(...)`/`csv.reader(...)`
    // called with no explicit dialect hit `GLOBAL_HASHMAP.get("excel").unwrap()`
    // inside rustpython-stdlib and panic the whole process. Found by
    // actually calling `csv.writer`, not by reading the source. Registering
    // it ourselves, with the same defaults CPython's `csv.py` uses, closes
    // the gap without needing the pure-Python wrapper at all.
    source.push_str(concat!(
        "class _rym_excel_dialect:\n",
        "    delimiter = ','\n",
        "    quotechar = '\"'\n",
        "    escapechar = None\n",
        "    doublequote = True\n",
        "    skipinitialspace = False\n",
        "    lineterminator = '\\r\\n'\n",
        "    quoting = 0\n",
        "    strict = False\n",
        "_csv.register_dialect('excel', _rym_excel_dialect)\n",
        "del _rym_excel_dialect\n",
    ));
    for (friendly, internal) in MODULE_ALIASES {
        source.push_str(&format!("_rym_alias_sys.modules['{friendly}'] = {internal}\n"));
    }
    source.push_str("del _rym_alias_sys\n");
    for (_, internal) in MODULE_ALIASES {
        source.push_str(&format!("del {internal}\n"));
    }
    let code = vm
        .compile(&source, compiler::Mode::Exec, "<rymscript-aliases>".to_string())
        .map_err(|err| vm.new_runtime_error(format!("{err}")))?;
    vm.run_code_obj(code, scope.clone())?;
    Ok(())
}

pub struct WinAppRunner {
    interpreter: Interpreter,
    scope: Scope,
}

impl WinAppRunner {
    /// Compiles and runs the script once (defining its globals/functions),
    /// then confirms it exposes a callable `draw`. Held alive for the
    /// life of the running app so module-level state persists across
    /// frames like a normal long-lived Python program would.
    pub fn load(source: &str, root: PathBuf) -> Result<Self> {
        let interpreter = build_interpreter();
        let scope = interpreter.enter(|vm| -> Result<Scope> {
            let scope = vm.new_scope_with_builtins();
            install_helpers(&scope, vm, root)
                .map_err(|exc| anyhow::anyhow!(format_exception(vm, exc)))?;
            let code = vm
                .compile(source, compiler::Mode::Exec, "<rymscript>".to_string())
                .map_err(|err| anyhow::anyhow!("{err}"))?;
            vm.run_code_obj(code, scope.clone())
                .map_err(|exc| anyhow::anyhow!(format_exception(vm, exc)))?;
            let has_draw = scope
                .globals
                .get_item_opt("draw", vm)
                .map_err(|exc| anyhow::anyhow!(format_exception(vm, exc)))?
                .is_some();
            if !has_draw {
                bail!("script must define a top-level `def draw(events):` function");
            }
            Ok(scope)
        })?;
        Ok(Self { interpreter, scope })
    }

    pub fn frame(&self, events: &HashMap<String, EventValue>) -> Result<Vec<Widget>> {
        self.interpreter.enter(|vm| -> Result<Vec<Widget>> {
            let draw_fn = self
                .scope
                .globals
                .get_item("draw", vm)
                .map_err(|exc| anyhow::anyhow!(format_exception(vm, exc)))?;
            let events_dict = vm.ctx.new_dict();
            for (key, value) in events {
                let py_value: PyObjectRef = match value {
                    EventValue::Clicked => vm.ctx.new_bool(true).into(),
                    EventValue::Text(text) => vm.ctx.new_str(text.clone()).into(),
                    EventValue::Bool(flag) => vm.ctx.new_bool(*flag).into(),
                };
                events_dict
                    .set_item(key.as_str(), py_value, vm)
                    .map_err(|exc| anyhow::anyhow!(format_exception(vm, exc)))?;
            }
            let result = draw_fn
                .call((events_dict,), vm)
                .map_err(|exc| anyhow::anyhow!(format_exception(vm, exc)))?;
            parse_widgets(vm, result).map_err(|exc| anyhow::anyhow!(format_exception(vm, exc)))
        })
    }
}

fn parse_widgets(vm: &VirtualMachine, obj: PyObjectRef) -> PyResult<Vec<Widget>> {
    let list = obj
        .downcast::<PyList>()
        .map_err(|_| vm.new_type_error("draw() must return a list of widget dicts".to_string()))?;
    parse_widget_list(vm, &list)
}

fn parse_widget_list(vm: &VirtualMachine, list: &PyList) -> PyResult<Vec<Widget>> {
    let mut widgets = Vec::new();
    for item in list.borrow_vec().iter() {
        widgets.push(parse_widget(vm, item.clone())?);
    }
    Ok(widgets)
}

fn parse_widget(vm: &VirtualMachine, item: PyObjectRef) -> PyResult<Widget> {
    let dict = item
        .downcast::<PyDict>()
        .map_err(|_| vm.new_type_error("each item returned by draw() must be a dict".to_string()))?;
    let widget_type = get_str(&dict, "type", vm)?;
    let widget = match widget_type.as_str() {
        "heading" => Widget::Heading(get_str(&dict, "text", vm)?),
        "label" => Widget::Label(get_str(&dict, "text", vm)?),
        "separator" => Widget::Separator,
        "button" => Widget::Button {
            id: get_str(&dict, "id", vm)?,
            text: get_str(&dict, "text", vm)?,
            color: get_color_opt(&dict, "color", vm)?,
            enabled: get_bool_opt(&dict, "enabled", vm)?.unwrap_or(true),
        },
        "text_input" => Widget::TextInput {
            id: get_str(&dict, "id", vm)?,
            value: get_str(&dict, "value", vm)?,
        },
        "checkbox" => Widget::Checkbox {
            id: get_str(&dict, "id", vm)?,
            text: get_str(&dict, "text", vm)?,
            value: get_bool(&dict, "value", vm)?,
        },
        "row" => Widget::Row(parse_children(&dict, vm)?),
        "column" => Widget::Column(parse_children(&dict, vm)?),
        "grid" => Widget::Grid {
            id: get_str(&dict, "id", vm)?,
            columns: get_usize(&dict, "columns", vm)?.max(1),
            children: parse_children(&dict, vm)?,
        },
        other => return Err(vm.new_value_error(format!("unknown widget type: {other}"))),
    };
    Ok(widget)
}

fn parse_children(dict: &PyDictRef, vm: &VirtualMachine) -> PyResult<Vec<Widget>> {
    let children = dict.get_item("children", vm)?;
    let list = children
        .downcast::<PyList>()
        .map_err(|_| vm.new_type_error("\"children\" must be a list of widget dicts".to_string()))?;
    parse_widget_list(vm, &list)
}

fn get_str(dict: &PyDictRef, key: &str, vm: &VirtualMachine) -> PyResult<String> {
    dict.get_item(key, vm)?.try_into_value(vm)
}

fn get_bool(dict: &PyDictRef, key: &str, vm: &VirtualMachine) -> PyResult<bool> {
    dict.get_item(key, vm)?.try_into_value(vm)
}

fn get_usize(dict: &PyDictRef, key: &str, vm: &VirtualMachine) -> PyResult<usize> {
    dict.get_item(key, vm)?.try_into_value(vm)
}

fn get_bool_opt(dict: &PyDictRef, key: &str, vm: &VirtualMachine) -> PyResult<Option<bool>> {
    let Some(value) = dict.get_item_opt(key, vm)? else { return Ok(None) };
    if vm.is_none(&value) {
        return Ok(None);
    }
    Ok(Some(value.try_into_value(vm)?))
}

/// A widget's optional `"color": [r, g, b]` field -- a plain 3-element
/// list of 0-255 ints rather than a hex string, so a script never needs
/// string parsing just to draw a colored button.
fn get_color_opt(dict: &PyDictRef, key: &str, vm: &VirtualMachine) -> PyResult<Option<[u8; 3]>> {
    let Some(value) = dict.get_item_opt(key, vm)? else { return Ok(None) };
    if vm.is_none(&value) {
        return Ok(None);
    }
    let list = value
        .downcast::<PyList>()
        .map_err(|_| vm.new_type_error(format!("\"{key}\" must be a [r, g, b] list of 0-255 ints")))?;
    let items = list.borrow_vec();
    if items.len() != 3 {
        return Err(vm.new_value_error(format!("\"{key}\" must have exactly 3 elements")));
    }
    let mut rgb = [0u8; 3];
    for (i, item) in items.iter().enumerate() {
        let n: i64 = item.clone().try_into_value(vm)?;
        rgb[i] = n.clamp(0, 255) as u8;
    }
    Ok(Some(rgb))
}

pub(crate) fn format_exception(vm: &VirtualMachine, exc: PyBaseExceptionRef) -> String {
    let mut buf: Vec<u8> = Vec::new();
    let _ = vm.write_exception(&mut rustpython_vm::py_io::IoWriter(&mut buf), &exc);
    String::from_utf8_lossy(&buf).trim_end().to_string()
}

/// The same sandboxed file API rymsh's Python REPL exposes
/// (read_file/write_file/append_file/list_dir/exists/remove_file/mkdir),
/// so a `.win` script can persist its own data using the identical
/// mental model a `.run` script uses -- only the UI half of the API
/// differs between the two program types.
pub(crate) fn install_helpers(scope: &Scope, vm: &VirtualMachine, root: PathBuf) -> PyResult<()> {
    install_module_aliases(scope, vm)?;

    macro_rules! set_fn {
        ($name:literal, $func:expr) => {
            scope
                .globals
                .set_item($name, vm.new_function($name, $func).into(), vm)?;
        };
    }

    {
        let root = root.clone();
        set_fn!("read_file", move |path: String, vm: &VirtualMachine| -> PyResult<String> {
            let real = sandbox_resolve(&root, &path);
            fs::read_to_string(&real).map_err(|err| vm.new_runtime_error(format!("{path}: {err}")))
        });
    }
    {
        let root = root.clone();
        set_fn!(
            "write_file",
            move |path: String, text: String, vm: &VirtualMachine| -> PyResult<()> {
                let real = sandbox_resolve(&root, &path);
                if let Some(parent) = real.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                fs::write(&real, text).map_err(|err| vm.new_runtime_error(format!("{path}: {err}")))
            }
        );
    }
    {
        let root = root.clone();
        set_fn!(
            "append_file",
            move |path: String, text: String, vm: &VirtualMachine| -> PyResult<()> {
                let real = sandbox_resolve(&root, &path);
                fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&real)
                    .and_then(|mut file| file.write_all(text.as_bytes()))
                    .map_err(|err| vm.new_runtime_error(format!("{path}: {err}")))
            }
        );
    }
    {
        let root = root.clone();
        set_fn!(
            "list_dir",
            move |path: OptionalArg<String>, vm: &VirtualMachine| -> PyResult<Vec<PyObjectRef>> {
                let input = path.into_option().unwrap_or_else(|| ".".to_string());
                let real = sandbox_resolve(&root, &input);
                let read_dir = fs::read_dir(&real)
                    .map_err(|err| vm.new_runtime_error(format!("{input}: {err}")))?;
                let mut names = Vec::new();
                for entry in read_dir {
                    let entry = entry.map_err(|err| vm.new_runtime_error(err.to_string()))?;
                    names.push(entry.file_name().to_string_lossy().to_string());
                }
                names.sort();
                Ok(names.into_iter().map(|name| vm.ctx.new_str(name).into()).collect())
            }
        );
    }
    {
        let root = root.clone();
        set_fn!("exists", move |path: String| -> bool { sandbox_resolve(&root, &path).exists() });
    }
    {
        let root = root.clone();
        set_fn!("remove_file", move |path: String, vm: &VirtualMachine| -> PyResult<()> {
            let real = sandbox_resolve(&root, &path);
            fs::remove_file(&real).map_err(|err| vm.new_runtime_error(format!("{path}: {err}")))
        });
    }
    {
        let root = root.clone();
        set_fn!("mkdir", move |path: String, vm: &VirtualMachine| -> PyResult<()> {
            let real = sandbox_resolve(&root, &path);
            fs::create_dir_all(&real).map_err(|err| vm.new_runtime_error(format!("{path}: {err}")))
        });
    }

    // Real Python's `json` module doesn't work here at all (see the doc
    // comment on `ALLOWED_STDLIB_MODULES`) -- these two functions give
    // the same practical capability, implemented directly against
    // `serde_json` rather than RustPython's incomplete `_json`
    // accelerator. Not `import json; json.loads(...)` -- `json_loads(...)`
    // and `json_dumps(...)` as plain functions, same naming convention as
    // `read_file`/`write_file`.
    set_fn!("json_loads", |text: String, vm: &VirtualMachine| -> PyResult<PyObjectRef> {
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|err| vm.new_value_error(format!("invalid JSON: {err}")))?;
        Ok(json_value_to_py(vm, &value))
    });
    set_fn!("json_dumps", |obj: PyObjectRef, vm: &VirtualMachine| -> PyResult<String> {
        let value = py_to_json_value(vm, &obj)?;
        serde_json::to_string(&value).map_err(|err| vm.new_value_error(format!("cannot serialize to JSON: {err}")))
    });

    // `time`/`datetime` aren't importable (see `ALLOWED_STDLIB_MODULES`'s
    // doc comment -- neither has real pure-Python wrapper support in this
    // RustPython build), so wall-clock time is hand-implemented against
    // `chrono` the same way `json_loads`/`json_dumps` stand in for
    // `json`: two plain functions, not a fake module. `now()` covers
    // "display the current time" (a clock); `now_ms()` covers "measure
    // elapsed time" (a stopwatch/timer), since naive epoch-second math in
    // a script has no other way to get a monotonic-ish timestamp.
    set_fn!("now", || -> String { chrono::Local::now().format("%H:%M:%S").to_string() });
    set_fn!("now_ms", || -> i64 { chrono::Utc::now().timestamp_millis() });

    Ok(())
}

fn json_value_to_py(vm: &VirtualMachine, value: &serde_json::Value) -> PyObjectRef {
    match value {
        serde_json::Value::Null => vm.ctx.none(),
        serde_json::Value::Bool(b) => vm.ctx.new_bool(*b).into(),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                vm.ctx.new_int(i).into()
            } else {
                vm.ctx.new_float(n.as_f64().unwrap_or(0.0)).into()
            }
        }
        serde_json::Value::String(s) => vm.ctx.new_str(s.as_str()).into(),
        serde_json::Value::Array(items) => {
            let converted: Vec<PyObjectRef> = items.iter().map(|item| json_value_to_py(vm, item)).collect();
            vm.ctx.new_list(converted).into()
        }
        serde_json::Value::Object(map) => {
            let dict = vm.ctx.new_dict();
            for (key, value) in map {
                let _ = dict.set_item(key.as_str(), json_value_to_py(vm, value), vm);
            }
            dict.into()
        }
    }
}

fn py_to_json_value(vm: &VirtualMachine, obj: &PyObjectRef) -> PyResult<serde_json::Value> {
    if vm.is_none(obj) {
        return Ok(serde_json::Value::Null);
    }
    // Order matters: Python's `bool` is a subclass of `int`, so the bool
    // check must run before the int check or every bool would come out
    // as 0/1 instead of true/false.
    if obj.class().is(vm.ctx.types.bool_type) {
        return Ok(serde_json::Value::Bool(obj.clone().try_to_bool(vm)?));
    }
    if let Some(int) = obj.downcast_ref::<rustpython_vm::builtins::PyInt>() {
        let value = int.as_bigint().to_string();
        return Ok(serde_json::Value::Number(
            value.parse::<i64>().map(serde_json::Number::from).unwrap_or_else(|_| serde_json::Number::from(0)),
        ));
    }
    if let Some(float) = obj.downcast_ref::<rustpython_vm::builtins::PyFloat>() {
        return Ok(serde_json::Number::from_f64(float.to_f64())
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null));
    }
    if let Some(s) = obj.downcast_ref::<rustpython_vm::builtins::PyStr>() {
        let text: &str = s.as_ref();
        return Ok(serde_json::Value::String(text.to_string()));
    }
    if let Some(list) = obj.downcast_ref::<PyList>() {
        let items = list
            .borrow_vec()
            .iter()
            .map(|item| py_to_json_value(vm, item))
            .collect::<PyResult<Vec<_>>>()?;
        return Ok(serde_json::Value::Array(items));
    }
    if let Some(dict) = obj.downcast_ref::<PyDict>() {
        let mut map = serde_json::Map::new();
        for (key, value) in dict {
            let key_py_str = key.str(vm)?;
            let key_str: &str = key_py_str.as_ref();
            map.insert(key_str.to_string(), py_to_json_value(vm, &value)?);
        }
        return Ok(serde_json::Value::Object(map));
    }
    Err(vm.new_type_error("object is not JSON-serializable".to_string()))
}

fn sandbox_resolve(root: &Path, input: &str) -> PathBuf {
    let mut result = root.to_path_buf();
    for component in Path::new(input.trim_start_matches('/')).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir if result.as_path() != root => {
                result.pop();
            }
            Component::ParentDir => {}
            Component::Normal(part) => result.push(part),
            _ => {}
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_root() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rymscript-test-{}-{}", std::process::id(), line!()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Collects every leaf widget (everything except `Row`/`Column`/
    /// `Grid` themselves) out of a possibly-nested tree, so tests can
    /// keep using the same flat `.iter().find_map(...)`/`.any(...)`
    /// patterns the pre-nesting tests already established, regardless of
    /// how deep a real app nests its containers.
    fn flatten_widgets(widgets: &[Widget]) -> Vec<&Widget> {
        let mut out = Vec::new();
        for widget in widgets {
            match widget {
                Widget::Row(children) | Widget::Column(children) => out.extend(flatten_widgets(children)),
                Widget::Grid { children, .. } => out.extend(flatten_widgets(children)),
                other => out.push(other),
            }
        }
        out
    }

    fn find_label(widgets: &[Widget]) -> Option<String> {
        flatten_widgets(widgets).into_iter().find_map(|w| match w {
            Widget::Label(text) => Some(text.clone()),
            _ => None,
        })
    }

    /// Exercises every real capability RYMSCRIPT_SDK.md promises, in its
    /// documented (sometimes non-standard) calling convention -- not
    /// just `import`, since a module that imports but whose methods
    /// don't actually work would pass a weaker test. This is the test
    /// that found the truth here: several modules import successfully
    /// but are missing the methods a script would actually call, which
    /// only showed up once real operations were attempted per module.
    #[test]
    fn allowed_stdlib_modules_actually_work() {
        let source = r##"
assert json_loads(json_dumps({"a": 1, "b": [1, 2, 3]}))["b"][2] == 3

import math
assert math.isclose(math.sqrt(16.0), 4.0)

import cmath
assert cmath.phase(complex(0, 1)) is not None

import random
r = random.Random()
assert 0 <= r.random() < 1

import sha256
digest = sha256.sha256(b"rymscript").hexdigest()
assert digest == "09d2517d74455d770c8ebbd6a6a5fedf2ef8f3ed9115566e0dd2047d34f53112", digest

import unicodedata
assert unicodedata.name("A") == "LATIN CAPITAL LETTER A"

import struct
assert struct.pack("i", 7) == struct.pack("i", 7)

import binascii
assert binascii.hexlify(b"ab") == b"6162"

import zlib
assert zlib.decompress(zlib.compress(b"hello")) == b"hello"

class Buf:
    def __init__(self):
        self.value = ""
    def write(self, s):
        self.value += s

import csv
buf = Buf()
writer = csv.writer(buf)
writer.writerow(["a", "b"])
assert "a,b" in buf.value

def draw(events):
    return [{"type": "label", "text": "all modules ok"}]
"##;
        let runner = WinAppRunner::load(source, scratch_root()).expect("load with curated stdlib");
        let widgets = runner.frame(&HashMap::new()).expect("frame");
        assert!(matches!(&widgets[0], Widget::Label(text) if text == "all modules ok"));
    }

    #[test]
    fn disallowed_modules_are_not_importable() {
        for module in ["os", "socket", "subprocess", "ctypes", "shutil", "json", "bisect", "hashlib"] {
            let source = format!(
                "import {module}\ndef draw(events):\n    return [{{\"type\": \"label\", \"text\": \"should not load\"}}]\n"
            );
            let result = WinAppRunner::load(&source, scratch_root());
            assert!(result.is_err(), "expected `import {module}` to fail, but the script loaded successfully");
        }
    }

    #[test]
    fn counter_starter_program_runs_and_reacts_to_clicks() {
        let source = include_str!("../../../starter_programs/counter.win");
        let runner = WinAppRunner::load(source, scratch_root()).expect("load counter.win");

        let widgets = runner.frame(&HashMap::new()).expect("first frame");
        let label_text = widgets.iter().find_map(|w| match w {
            Widget::Label(text) => Some(text.clone()),
            _ => None,
        });
        assert_eq!(label_text.as_deref(), Some("Count: 0"), "counter should start at 0");
        assert!(
            widgets.iter().any(|w| matches!(w, Widget::Button { id, .. } if id == "inc")),
            "should expose an Increment button"
        );

        let mut events = HashMap::new();
        events.insert("inc".to_string(), EventValue::Clicked);
        let widgets = runner.frame(&events).expect("second frame after click");
        let label_text = widgets.iter().find_map(|w| match w {
            Widget::Label(text) => Some(text.clone()),
            _ => None,
        });
        assert_eq!(label_text.as_deref(), Some("Count: 1"), "count should persist and increment across frames");

        let mut events = HashMap::new();
        events.insert("reset".to_string(), EventValue::Clicked);
        let widgets = runner.frame(&events).expect("third frame after reset");
        let label_text = widgets.iter().find_map(|w| match w {
            Widget::Label(text) => Some(text.clone()),
            _ => None,
        });
        assert_eq!(label_text.as_deref(), Some("Count: 0"), "reset should zero the counter");
    }

    #[test]
    fn notes_starter_program_runs_and_persists_across_frames() {
        let source = include_str!("../../../starter_programs/notes.win");
        let runner = WinAppRunner::load(source, scratch_root()).expect("load notes.win");

        let widgets = runner.frame(&HashMap::new()).expect("first frame");
        assert!(
            widgets.iter().any(|w| matches!(w, Widget::TextInput { id, .. } if id == "draft_0")),
            "should draw the initial draft field"
        );

        let mut events = HashMap::new();
        events.insert("draft_0".to_string(), EventValue::Text("buy milk".to_string()));
        events.insert("add".to_string(), EventValue::Clicked);
        let widgets = runner.frame(&events).expect("frame after adding a note");
        assert!(
            widgets.iter().any(|w| matches!(w, Widget::Checkbox { id, text, value } if id == "done_0" && text == "buy milk" && !value)),
            "the new note should render as an unchecked checkbox"
        );

        // The id bump only takes effect on the *next* draw() call, since
        // this frame's widget list was already built before the add-note
        // branch ran and incremented `draft_generation`.
        let widgets = runner.frame(&HashMap::new()).expect("frame after the id bump takes effect");
        assert!(
            widgets.iter().any(|w| matches!(w, Widget::TextInput { id, .. } if id == "draft_1")),
            "adding a note should bump the draft field's id to clear it"
        );

        let mut events = HashMap::new();
        events.insert("done_0".to_string(), EventValue::Bool(true));
        let widgets = runner.frame(&events).expect("frame after checking the note done");
        assert!(
            widgets.iter().any(|w| matches!(w, Widget::Checkbox { id, text, value } if id == "done_0" && text == "[done] buy milk" && *value)),
            "checking a note off should persist and relabel it"
        );
    }

    #[test]
    fn rejects_scripts_without_draw() {
        let result = WinAppRunner::load("x = 1", scratch_root());
        assert!(result.is_err(), "a script with no draw() should fail to load");
    }

    #[test]
    fn file_helpers_are_sandboxed_to_root() {
        let source = r#"
write_file("note.txt", "hello from rymscript")
saved = read_file("note.txt")

def draw(events):
    return [{"type": "label", "text": saved}]
"#;
        let root = scratch_root();
        let runner = WinAppRunner::load(source, root.clone()).expect("load");
        let widgets = runner.frame(&HashMap::new()).expect("frame");
        assert!(matches!(&widgets[0], Widget::Label(text) if text == "hello from rymscript"));
        assert!(root.join("note.txt").exists(), "write_file should land under the sandbox root");
    }

    #[test]
    fn row_and_column_containers_nest_widgets() {
        let source = r#"
def draw(events):
    return [
        {"type": "row", "children": [
            {"type": "button", "id": "a", "text": "A"},
            {"type": "column", "children": [
                {"type": "button", "id": "b", "text": "B"},
            ]},
        ]},
    ]
"#;
        let runner = WinAppRunner::load(source, scratch_root()).expect("load");
        let widgets = runner.frame(&HashMap::new()).expect("frame");
        let Widget::Row(children) = &widgets[0] else { panic!("expected a Row widget") };
        assert_eq!(children.len(), 2);
        assert!(matches!(&children[0], Widget::Button { id, .. } if id == "a"));
        let Widget::Column(inner) = &children[1] else { panic!("expected a nested Column widget") };
        assert!(matches!(&inner[0], Widget::Button { id, .. } if id == "b"));
    }

    #[test]
    fn grid_container_parses_columns_and_children() {
        let source = r#"
def draw(events):
    return [
        {"type": "grid", "id": "g", "columns": 2, "children": [
            {"type": "button", "id": "a", "text": "A"},
            {"type": "button", "id": "b", "text": "B"},
            {"type": "button", "id": "c", "text": "C"},
        ]},
    ]
"#;
        let runner = WinAppRunner::load(source, scratch_root()).expect("load");
        let widgets = runner.frame(&HashMap::new()).expect("frame");
        let Widget::Grid { id, columns, children } = &widgets[0] else { panic!("expected a Grid widget") };
        assert_eq!(id, "g");
        assert_eq!(*columns, 2);
        assert_eq!(children.len(), 3);
    }

    #[test]
    fn button_color_and_enabled_default_when_omitted() {
        let source = r#"
def draw(events):
    return [{"type": "button", "id": "a", "text": "A"}]
"#;
        let runner = WinAppRunner::load(source, scratch_root()).expect("load");
        let widgets = runner.frame(&HashMap::new()).expect("frame");
        let Widget::Button { color, enabled, .. } = &widgets[0] else { panic!("expected a Button widget") };
        assert_eq!(*color, None);
        assert!(*enabled, "enabled should default to true when omitted, so existing scripts render unchanged");
    }

    #[test]
    fn button_color_and_enabled_parse_when_present() {
        let source = r#"
def draw(events):
    return [{"type": "button", "id": "a", "text": "A", "color": [10, 20, 30], "enabled": False}]
"#;
        let runner = WinAppRunner::load(source, scratch_root()).expect("load");
        let widgets = runner.frame(&HashMap::new()).expect("frame");
        let Widget::Button { color, enabled, .. } = &widgets[0] else { panic!("expected a Button widget") };
        assert_eq!(*color, Some([10, 20, 30]));
        assert!(!*enabled);
    }

    #[test]
    fn now_and_now_ms_are_callable_and_well_formed() {
        let source = r#"
t = now()
ms = now_ms()
assert isinstance(t, str) and len(t) == 8, t   # "HH:MM:SS"
assert isinstance(ms, int) and ms > 0, ms

def draw(events):
    return [{"type": "label", "text": t}]
"#;
        let runner = WinAppRunner::load(source, scratch_root()).expect("load");
        runner.frame(&HashMap::new()).expect("frame");
    }

    #[test]
    fn clock_starter_program_ticks_and_runs_a_stopwatch() {
        let source = include_str!("../../../starter_programs/clock.win");
        let runner = WinAppRunner::load(source, scratch_root()).expect("load clock.win");

        let widgets = runner.frame(&HashMap::new()).expect("first frame");
        let flat = flatten_widgets(&widgets);
        assert!(
            flat.iter().any(|w| matches!(w, Widget::Button { id, text, .. } if id == "start_stop" && text == "Start")),
            "stopwatch should start in the stopped state"
        );

        let mut events = HashMap::new();
        events.insert("start_stop".to_string(), EventValue::Clicked);
        let widgets = runner.frame(&events).expect("frame after starting");
        let flat = flatten_widgets(&widgets);
        assert!(
            flat.iter().any(|w| matches!(w, Widget::Button { id, text, .. } if id == "start_stop" && text == "Stop")),
            "starting the stopwatch should flip the button to Stop"
        );
        assert!(
            flat.iter().any(|w| matches!(w, Widget::Button { id, enabled, .. } if id == "reset" && !enabled)),
            "reset should be disabled while the stopwatch is running"
        );
    }

    #[test]
    fn calculator_starter_program_computes_seven_plus_three() {
        let source = include_str!("../../../starter_programs/calculator.win");
        let runner = WinAppRunner::load(source, scratch_root()).expect("load calculator.win");

        let click = |id: &str| -> Vec<Widget> {
            let mut events = HashMap::new();
            events.insert(id.to_string(), EventValue::Clicked);
            runner.frame(&events).expect("frame")
        };

        click("digit_7");
        click("op_+");
        click("digit_3");
        let widgets = click("equals");
        assert_eq!(find_label(&widgets).as_deref(), Some("10"), "7 + 3 should equal 10");

        // Clear should reset the display back to the initial state.
        let widgets = click("clear");
        assert_eq!(find_label(&widgets).as_deref(), Some("0"));
    }

    #[test]
    fn minesweeper_starter_program_renders_a_full_board_and_flags_without_revealing() {
        let source = include_str!("../../../starter_programs/minesweeper.win");
        let runner = WinAppRunner::load(source, scratch_root()).expect("load minesweeper.win");

        let widgets = runner.frame(&HashMap::new()).expect("first frame");
        let flat = flatten_widgets(&widgets);
        let cell_count = flat.iter().filter(|w| matches!(w, Widget::Button { id, .. } if id.starts_with("cell_"))).count();
        assert_eq!(cell_count, 64, "an 8x8 board should have 64 cell buttons");
        assert!(
            flat.iter().all(|w| !matches!(w, Widget::Button { id, text, .. } if id.starts_with("cell_") && !text.is_empty())),
            "no cell should show anything before it's been revealed or flagged"
        );

        // Flagging never depends on where the mines actually are, so
        // this is deterministic regardless of the random layout.
        let mut events = HashMap::new();
        events.insert("flag_mode".to_string(), EventValue::Bool(true));
        events.insert("cell_0_0".to_string(), EventValue::Clicked);
        let widgets = runner.frame(&events).expect("frame after flagging");
        let flat = flatten_widgets(&widgets);
        assert!(
            flat.iter().any(|w| matches!(w, Widget::Button { id, text, .. } if id == "cell_0_0" && text == "F")),
            "flag mode should flag the clicked cell instead of revealing it"
        );

        // Restart should always produce a fresh, fully-hidden board.
        let mut events = HashMap::new();
        events.insert("restart".to_string(), EventValue::Clicked);
        let widgets = runner.frame(&events).expect("frame after restart");
        let flat = flatten_widgets(&widgets);
        assert!(
            flat.iter().all(|w| !matches!(w, Widget::Button { id, text, .. } if id.starts_with("cell_") && !text.is_empty())),
            "restart should clear every flag/reveal"
        );
    }
}
