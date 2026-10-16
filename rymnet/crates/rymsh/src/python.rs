use crate::vfs::VirtualComputer;
use anyhow::{Context, Result};
use rustpython_vm::builtins::{PyDict, PyList};
use rustpython_vm::convert::IntoPyException;
use rustpython_vm::function::OptionalArg;
use rustpython_vm::scope::Scope;
use rustpython_vm::{AsObject, Interpreter, PyObjectRef, PyResult, VirtualMachine, compiler};
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;
use std::fs;
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};

const BANNER: &str = "rymsh python (RustPython) -- a small sandboxed interpreter for fun/small scripts.\n\
Only your virtual computer is reachable: no real filesystem, network, or process access.\n\
File helpers: read_file(path), write_file(path, text), append_file(path, text),\n              list_dir(path='.'), exists(path), remove_file(path), mkdir(path)\n\
JSON helpers: json_loads(text), json_dumps(obj)\n\
Curated stdlib: math, cmath, unicodedata, zlib, binascii, csv, struct, random\n              (random.Random().random(), not random.random()), and per-algorithm\n              hashing (sha256.sha256(data), not hashlib.sha256(data)) -- see\n              RYMSCRIPT_SDK.md for the full list and why.\n\
An empty line ends an indented block (if/for/def/...). exit() or Ctrl+D leaves.\n";

/// See the identically-named consts/functions in `rymwin`'s `rymscript.rs`
/// for the full rationale (which modules were tried and dropped, and why).
/// Duplicated here rather than shared because `rymsh` and `rymwin` are
/// separate binaries and this project's existing pattern is per-crate
/// duplication over a shared internal crate.
const ALLOWED_STDLIB_MODULES: &[&str] = &[
    "math",
    "cmath",
    "unicodedata",
    "zlib",
    "binascii",
    "_csv",
    "_struct",
    "_random",
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
/// registered, plus `MODULE_ALIASES` applied so scripts can `import struct`
/// rather than needing to know it's really `_struct`.
fn build_interpreter() -> Interpreter {
    let builder = Interpreter::builder(Default::default());
    let ctx = builder.ctx.clone();
    let modules: Vec<&'static rustpython_vm::builtins::PyModuleDef> = rustpython_stdlib::stdlib_module_defs(&ctx)
        .into_iter()
        .filter(|def| ALLOWED_STDLIB_MODULES.contains(&def.name.as_str()))
        .collect();
    builder.add_native_modules(&modules).build()
}

/// Registers each `MODULE_ALIASES` friendly name in `sys.modules` and
/// bootstraps `csv`'s missing "excel" default dialect. Has to run as
/// compiled Python source through `run_code_obj`, not a bare Rust-side
/// `vm.import()` call -- see `rymwin`'s `rymscript.rs` for why.
fn install_module_aliases(scope: &Scope, vm: &VirtualMachine) -> PyResult<()> {
    let mut source = String::from("import sys as _rym_alias_sys\n");
    for (_, internal) in MODULE_ALIASES {
        source.push_str(&format!("import {internal}\n"));
    }
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

/// Runs an interactive Python REPL backed by a persistent scope, so
/// variables/functions defined on one line are visible on the next -- like a
/// normal `python3` REPL, except `read_file`/`write_file`/etc are the only
/// way in or out, and they never leave the current vfs.
pub fn repl(vfs: &VirtualComputer) -> Result<()> {
    println!("{BANNER}");
    let root = vfs.root.clone();
    let cwd = vfs.cwd.clone();

    build_interpreter().enter(|vm| -> Result<()> {
        let scope = vm.new_scope_with_builtins();
        install_module_aliases(&scope, vm)
            .map_err(|exc| anyhow::anyhow!(format_exception(vm, exc)))?;
        install_helpers(&scope, vm, root, cwd)
            .map_err(|exc| anyhow::anyhow!(format_exception(vm, exc)))?;

        let mut rl = DefaultEditor::new().context("start python line editor")?;
        let mut buffer = String::new();
        let mut in_block = false;

        loop {
            let prompt = if in_block { "... " } else { ">>> " };
            match rl.readline(prompt) {
                Ok(line) => {
                    if !in_block {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        if trimmed == "exit()" || trimmed == "quit()" {
                            break;
                        }
                        let _ = rl.add_history_entry(&line);
                        if trimmed.ends_with(':') {
                            buffer.push_str(&line);
                            buffer.push('\n');
                            in_block = true;
                        } else {
                            run(vm, &scope, &line, compiler::Mode::Single);
                        }
                    } else {
                        let _ = rl.add_history_entry(&line);
                        if line.trim().is_empty() {
                            run(vm, &scope, &buffer, compiler::Mode::Exec);
                            buffer.clear();
                            in_block = false;
                        } else {
                            buffer.push_str(&line);
                            buffer.push('\n');
                        }
                    }
                }
                Err(ReadlineError::Interrupted) => {
                    buffer.clear();
                    in_block = false;
                    continue;
                }
                Err(ReadlineError::Eof) => break,
                Err(_) => break,
            }
        }
        Ok(())
    })
}

/// Runs a whole `.py` file from the vfs in one shot (no REPL).
pub fn run_file(vfs: &VirtualComputer, script: &str) -> Result<()> {
    let path = vfs.real_path(script);
    let source = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let root = vfs.root.clone();
    let cwd = vfs.cwd.clone();

    build_interpreter().enter(|vm| -> Result<()> {
        let scope = vm.new_scope_with_builtins();
        install_module_aliases(&scope, vm)
            .map_err(|exc| anyhow::anyhow!(format_exception(vm, exc)))?;
        install_helpers(&scope, vm, root, cwd)
            .map_err(|exc| anyhow::anyhow!(format_exception(vm, exc)))?;
        run(vm, &scope, &source, compiler::Mode::Exec);
        Ok(())
    })
}

fn run(vm: &VirtualMachine, scope: &Scope, source: &str, mode: compiler::Mode) {
    let result = vm
        .compile(source, mode, "<rymsh>".to_string())
        .map_err(|err| (err, Some(source)).into_pyexception(vm))
        .and_then(|code| vm.run_code_obj(code, scope.clone()));
    if let Err(exc) = result {
        vm.print_exception(exc);
    }
}

fn format_exception(
    vm: &VirtualMachine,
    exc: rustpython_vm::builtins::PyBaseExceptionRef,
) -> String {
    vm.print_exception(exc);
    "failed to start the python sandbox".to_string()
}

fn install_helpers(
    scope: &Scope,
    vm: &VirtualMachine,
    root: PathBuf,
    cwd: PathBuf,
) -> PyResult<()> {
    macro_rules! set_fn {
        ($name:literal, $func:expr) => {
            scope
                .globals
                .set_item($name, vm.new_function($name, $func).into(), vm)?;
        };
    }

    {
        let (root, cwd) = (root.clone(), cwd.clone());
        set_fn!("read_file", move |path: String,
                                   vm: &VirtualMachine|
              -> PyResult<String> {
            let real = sandbox_resolve(&root, &cwd, &path);
            fs::read_to_string(&real).map_err(|err| vm.new_runtime_error(format!("{path}: {err}")))
        });
    }
    {
        let (root, cwd) = (root.clone(), cwd.clone());
        set_fn!("write_file", move |path: String,
                                    text: String,
                                    vm: &VirtualMachine|
              -> PyResult<()> {
            let real = sandbox_resolve(&root, &cwd, &path);
            fs::write(&real, text).map_err(|err| vm.new_runtime_error(format!("{path}: {err}")))
        });
    }
    {
        let (root, cwd) = (root.clone(), cwd.clone());
        set_fn!("append_file", move |path: String,
                                     text: String,
                                     vm: &VirtualMachine|
              -> PyResult<()> {
            let real = sandbox_resolve(&root, &cwd, &path);
            fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&real)
                .and_then(|mut file| file.write_all(text.as_bytes()))
                .map_err(|err| vm.new_runtime_error(format!("{path}: {err}")))
        });
    }
    {
        let (root, cwd) = (root.clone(), cwd.clone());
        set_fn!("list_dir", move |path: OptionalArg<String>,
                                  vm: &VirtualMachine|
              -> PyResult<Vec<PyObjectRef>> {
            let input = path.into_option().unwrap_or_else(|| ".".to_string());
            let real = sandbox_resolve(&root, &cwd, &input);
            let read_dir = fs::read_dir(&real)
                .map_err(|err| vm.new_runtime_error(format!("{input}: {err}")))?;
            let mut names = Vec::new();
            for entry in read_dir {
                let entry = entry.map_err(|err| vm.new_runtime_error(err.to_string()))?;
                names.push(entry.file_name().to_string_lossy().to_string());
            }
            names.sort();
            Ok(names
                .into_iter()
                .map(|name| vm.ctx.new_str(name).into())
                .collect())
        });
    }
    {
        let (root, cwd) = (root.clone(), cwd.clone());
        set_fn!("exists", move |path: String| -> bool {
            sandbox_resolve(&root, &cwd, &path).exists()
        });
    }
    {
        let (root, cwd) = (root.clone(), cwd.clone());
        set_fn!("remove_file", move |path: String,
                                     vm: &VirtualMachine|
              -> PyResult<()> {
            let real = sandbox_resolve(&root, &cwd, &path);
            fs::remove_file(&real).map_err(|err| vm.new_runtime_error(format!("{path}: {err}")))
        });
    }
    {
        let (root, cwd) = (root.clone(), cwd.clone());
        set_fn!("mkdir", move |path: String,
                               vm: &VirtualMachine|
              -> PyResult<()> {
            let real = sandbox_resolve(&root, &cwd, &path);
            fs::create_dir_all(&real).map_err(|err| vm.new_runtime_error(format!("{path}: {err}")))
        });
    }

    // Real Python's `json` module doesn't work here at all (RustPython's
    // native `_json` is an accelerator missing `.loads`/`.dumps` without
    // CPython's pure-Python wrapper) -- these two functions give the same
    // practical capability, implemented directly against `serde_json`
    // rather than that incomplete module. Not `import json; json.loads(...)`
    // -- `json_loads(...)` and `json_dumps(...)` as plain functions, same
    // naming convention as `read_file`/`write_file`.
    set_fn!("json_loads", |text: String, vm: &VirtualMachine| -> PyResult<PyObjectRef> {
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|err| vm.new_value_error(format!("invalid JSON: {err}")))?;
        Ok(json_value_to_py(vm, &value))
    });
    set_fn!("json_dumps", |obj: PyObjectRef, vm: &VirtualMachine| -> PyResult<String> {
        let value = py_to_json_value(vm, &obj)?;
        serde_json::to_string(&value).map_err(|err| vm.new_value_error(format!("cannot serialize to JSON: {err}")))
    });

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
    // check must run before the int check or every bool would come out as
    // 0/1 instead of true/false.
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

/// Mirrors `VirtualComputer`'s own path clamping: relative paths resolve
/// against the sandbox cwd captured when the interpreter started, absolute
/// (`/...`) paths resolve against the sandbox root, and `..` can never climb
/// above that root. There is no way for a script to name a path outside it.
fn sandbox_resolve(root: &Path, cwd: &Path, input: &str) -> PathBuf {
    let base = if input.starts_with('/') {
        root.to_path_buf()
    } else {
        cwd.to_path_buf()
    };
    let mut result = base;
    for component in Path::new(input.trim_start_matches('/')).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir if result != root => {
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
    use crate::vfs::VirtualComputer;

    /// Exercises the curated stdlib end to end through the real `run_file`
    /// entry point (not just `build_interpreter` in isolation), writing
    /// results back out through `write_file` since `run_file`'s own
    /// `print()` output goes to the real stdout, not anything a test can
    /// capture.
    #[test]
    fn allowed_stdlib_modules_actually_work() {
        let root = std::env::temp_dir().join(format!("rymsh-python-stdlib-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let vfs = VirtualComputer::open(root.clone()).expect("open vfs");

        let script = vfs.real_path("check.py");
        fs::write(
            &script,
            r##"
import math
import cmath
import random
import sha256
import unicodedata
import struct
import binascii
import zlib
import csv

class Buf:
    def __init__(self):
        self.value = ""
    def write(self, s):
        self.value += s

buf = Buf()
writer = csv.writer(buf)
writer.writerow(["a", "b"])

r = random.Random()

results = {
    "sqrt": math.isclose(math.sqrt(16.0), 4.0),
    "phase": cmath.phase(complex(0, 1)) is not None,
    "random": 0 <= r.random() < 1,
    "sha256": sha256.sha256(b"rymscript").hexdigest(),
    "unicodedata": unicodedata.name("A"),
    "struct": struct.pack("i", 7) == struct.pack("i", 7),
    "binascii": binascii.hexlify(b"ab") == b"6162",
    "zlib": zlib.decompress(zlib.compress(b"hello")) == b"hello",
    "csv": "a,b" in buf.value,
}
write_file("result.json", json_dumps(results))
"##,
        )
        .expect("write script");

        super::run_file(&vfs, "check.py").expect("run_file");

        let result_text = fs::read_to_string(vfs.real_path("result.json")).expect("read result.json");
        let result: serde_json::Value = serde_json::from_str(&result_text).expect("parse result.json");
        assert_eq!(result["sqrt"], serde_json::json!(true));
        assert_eq!(result["phase"], serde_json::json!(true));
        assert_eq!(result["random"], serde_json::json!(true));
        assert_eq!(
            result["sha256"],
            serde_json::json!("09d2517d74455d770c8ebbd6a6a5fedf2ef8f3ed9115566e0dd2047d34f53112")
        );
        assert_eq!(result["unicodedata"], serde_json::json!("LATIN CAPITAL LETTER A"));
        assert_eq!(result["struct"], serde_json::json!(true));
        assert_eq!(result["binascii"], serde_json::json!(true));
        assert_eq!(result["zlib"], serde_json::json!(true));
        assert_eq!(result["csv"], serde_json::json!(true));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn disallowed_modules_are_not_importable() {
        let root = std::env::temp_dir().join(format!("rymsh-python-disallowed-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let vfs = VirtualComputer::open(root.clone()).expect("open vfs");

        for module in ["os", "socket", "subprocess", "ctypes", "shutil", "json", "bisect", "hashlib"] {
            let script = vfs.real_path("check.py");
            fs::write(&script, format!("import {module}\nwrite_file('ok.txt', 'yes')\n")).expect("write script");
            let _ = fs::remove_file(vfs.real_path("ok.txt"));
            let _ = super::run_file(&vfs, "check.py");
            assert!(
                !vfs.real_path("ok.txt").exists(),
                "expected `import {module}` to fail, but the script ran to completion"
            );
        }

        let _ = fs::remove_dir_all(&root);
    }
}
