//! Python / componentize-py plugin scaffold and build pipeline.

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

use super::{Finding, prompt, write_file};

// ── Templates ─────────────────────────────────────────────────────────────────

/// Full WASI compatibility shim for FastMCP (anyio-based) plugins.
const WASM_ENTRY_FASTMCP: &str = r#""""WASI compatibility shim — entry point for componentize-py.

This module MUST be the componentize-py entry (not app.py).
It applies all mandatory patches before any application code runs.
"""

# ── 1. Force anyio asyncio backend into the bundle ───────────────────────────
# anyio loads backends by string at runtime; the bundler never sees this.
# Without it: ModuleNotFoundError: No module named 'anyio._backends'
import anyio._backends._asyncio  # noqa: F401

# ── 2. Patch asyncio self-pipe ────────────────────────────────────────────────
# asyncio calls socketpair() internally for inter-thread wake-ups.
# socketpair() does not exist in WASI P2 — wasi-libc aborts without this patch.
import asyncio.selector_events as _se


class _DummySock:
    def fileno(self): return -1
    def close(self): pass
    def write(self, b): pass
    def flush(self): pass
    def read(self, n): return b""


def _wasi_make_self_pipe(self):
    self._ssock = _DummySock()
    self._csock = _DummySock()
    self._internal_fds += 1


def _wasi_close_self_pipe(self):
    for attr in ("_ssock", "_csock"):
        s = getattr(self, attr, None)
        if s is not None:
            s.close()
            setattr(self, attr, None)
    self._internal_fds -= 1


_se.BaseSelectorEventLoop._make_self_pipe  = _wasi_make_self_pipe
_se.BaseSelectorEventLoop._close_self_pipe = _wasi_close_self_pipe
_se.BaseSelectorEventLoop._write_to_self   = lambda self: None
_se.BaseSelectorEventLoop._read_from_self  = lambda self: None

# ── 3. Patch anyio thread runner ─────────────────────────────────────────────
# WASM has no OS threads. anyio.to_thread.run_sync would trap.
# Replace with inline synchronous execution (safe for single-connection stdio).
import anyio.to_thread as _att
import anyio._backends._asyncio as _anyio_asyncio


async def _wasm_run_sync(func, *args, limiter=None, cancellable=False,
                          abandon_on_cancel=None):
    return func(*args)


_att.run_sync = _wasm_run_sync
_anyio_asyncio.run_sync_in_worker_thread = _wasm_run_sync

# ── Re-export Run for wasi:cli/run@0.2.0 ─────────────────────────────────────
from app import Run  # noqa: E402

__all__ = ["Run"]
"#;

/// Minimal shim for pure Python plugins — no asyncio/anyio patches needed.
const WASM_ENTRY_PURE: &str = r#""""WASI shim entry point for componentize-py.

Pure Python MCP server — no asyncio/anyio dependency, no patches needed.
"""
from app import Run

__all__ = ["Run"]
"#;

const WIT_WORLD: &str = r#"package mcp:plugin;

world plugin {
  include wasi:cli/imports@0.2.0;
  export wasi:cli/run@0.2.0;
}
"#;

const GITIGNORE: &str = r#"__pycache__/
*.pyc
*.wasm
.venv/
dist/
"#;

const REQUIREMENTS_FASTMCP: &str = "mcp[server]>=1.6\nanyio>=4.5\n";

const REQUIREMENTS_PURE: &str = r#"# No runtime dependencies — pure Python MCP server.
# Build-time only (install in your virtualenv before running `craft mcp build`):
#   pip install componentize-py
"#;

// Templates use __PLUGIN_NAME__ as a placeholder — no format! escaping needed.
// A plain .replace() at write-time substitutes the actual name.

const APP_PY_FASTMCP: &str = r#""""MCP plugin — __PLUGIN_NAME__.

Built with FastMCP. Entry point is wasm_entry.py (not this file).
"""
import logging

from mcp.server.fastmcp import FastMCP

logger = logging.getLogger(__name__)
mcp = FastMCP(name="__PLUGIN_NAME__")


# ── Tools ─────────────────────────────────────────────────────────────────────

@mcp.tool()
def hello(name: str) -> str:
    """Say hello to someone."""
    return f"Hello, {name}!"


# ── Run ───────────────────────────────────────────────────────────────────────

class Run:
    """componentize-py entry point for wasi:cli/run@0.2.0."""

    def run(self) -> None:
        logging.basicConfig(level=logging.INFO,
                            format="%(name)s - %(levelname)s - %(message)s")
        mcp.run(transport="stdio")


if __name__ == "__main__":
    Run().run()
"#;

const APP_PY_PURE: &str = r#""""MCP plugin — __PLUGIN_NAME__.

Pure Python MCP stdio server. Entry point is wasm_entry.py (not this file).
No external dependencies — reads/writes newline-delimited JSON-RPC 2.0.
"""
import json
import sys


def handle(req: dict) -> "dict | None":
    method = req.get("method", "")
    req_id = req.get("id")

    if method == "initialize":
        return {
            "jsonrpc": "2.0", "id": req_id,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "__PLUGIN_NAME__", "version": "0.1.0"},
            },
        }

    if method == "initialized":
        return None  # notification — no response

    if method == "tools/list":
        return {
            "jsonrpc": "2.0", "id": req_id,
            "result": {"tools": [
                {
                    "name": "hello",
                    "description": "Say hello to someone.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {"name": {"type": "string", "description": "Who to greet"}},
                        "required": ["name"],
                    },
                },
            ]},
        }

    if method == "tools/call":
        params = req.get("params", {})
        tool   = params.get("name", "")
        args   = params.get("arguments", {})
        if tool == "hello":
            return {
                "jsonrpc": "2.0", "id": req_id,
                "result": {"content": [{"type": "text", "text": f"Hello, {args.get('name', 'world')}!"}]},
            }
        return {
            "jsonrpc": "2.0", "id": req_id,
            "error": {"code": -32602, "message": f"Unknown tool: {tool}"},
        }

    if req_id is not None:
        return {
            "jsonrpc": "2.0", "id": req_id,
            "error": {"code": -32601, "message": f"Method not found: {method}"},
        }
    return None


def run() -> None:
    buf = ""
    while True:
        chunk = sys.stdin.read(1)
        if not chunk:
            break
        buf += chunk
        if buf.endswith("\n"):
            line = buf.strip()
            buf = ""
            if not line:
                continue
            try:
                resp = handle(json.loads(line))
                if resp is not None:
                    sys.stdout.write(json.dumps(resp) + "\n")
                    sys.stdout.flush()
            except Exception:
                pass


class Run:
    """componentize-py entry point for wasi:cli/run@0.2.0."""

    def run(self) -> None:
        run()


if __name__ == "__main__":
    run()
"#;

const MANIFEST_TOML: &str = r#"name            = "__PLUGIN_NAME__"
kind            = "wasm"
source          = "__PLUGIN_NAME__.wasm"
source_hash     = ""
env_vars        = []
allowed_domains = []
"#;

fn fill(template: &str, name: &str) -> String {
    template.replace("__PLUGIN_NAME__", name)
}

// ── Embedded Python AST analyser ─────────────────────────────────────────────
//
// Invoked as: python3 -c ANALYSER_PY file1.py file2.py ...
// Outputs a JSON array of findings to stdout.
// Using Python's own ast module means:
//   - string literals, docstrings, and comments never trigger false positives
//   - real line numbers from the parsed AST
//   - correct handling of aliases (import threading as t)

const ANALYSER_PY: &str = r#"
import ast, json, sys

# ── Rule tables ──────────────────────────────────────────────────────────────
# Each entry: (module_prefix, message, is_error)
IMPORT_RULES = [
    ("threading",          "threads not available in WASM — use async instead",                       True),
    ("subprocess",         "subprocesses not available in WASM",                                       True),
    ("multiprocessing",    "multiprocessing not available in WASM",                                    True),
    ("concurrent.futures", "thread pools not available in WASM",                                       True),
    ("_thread",            "threads not available in WASM",                                            True),
    ("requests",           "requests uses raw sockets; use httpx + WASI transport instead",            False),
    ("aiohttp",            "aiohttp uses raw sockets; not WASI P2 compatible",                        False),
    ("urllib3",            "urllib3 uses raw sockets; not WASI P2 compatible",                        False),
]

# Each entry: (obj_name, method_name, message, is_error)
ATTR_CALL_RULES = [
    ("threading",  "Thread",                  "threads not available in WASM",                                              True),
    ("threading",  "Timer",                   "threads not available in WASM",                                              True),
    ("os",         "fork",                    "os.fork not available in WASM",                                              True),
    ("asyncio",    "create_subprocess_exec",  "subprocesses not available in WASM",                                         True),
    ("asyncio",    "create_subprocess_shell", "subprocesses not available in WASM",                                         True),
    ("asyncio",    "to_thread",               "patched by wasm_entry.py — ensure shim is the componentize-py entry",        False),
    ("socket",     "socketpair",              "patched by wasm_entry.py — ensure shim is the componentize-py entry",        False),
    ("pathlib",    "Path",                    "filesystem not available in WASM (no preopened dirs)",                       False),
]

# Each entry: (func_name, message, is_error)
NAME_CALL_RULES = [
    ("open", "filesystem not available in WASM (no preopened dirs)", False),
]

# ── Visitor ───────────────────────────────────────────────────────────────────

class Checker(ast.NodeVisitor):
    def __init__(self, path, lines):
        self.path = path
        self.lines = lines
        self.findings = []

    def _add(self, node, message, is_error):
        lineno = getattr(node, "lineno", 0)
        source = self.lines[lineno - 1].strip() if 0 < lineno <= len(self.lines) else ""
        self.findings.append({
            "file": self.path,
            "line": lineno,
            "message": message,
            "is_error": is_error,
            "source": source,
        })

    def visit_Import(self, node):
        for alias in node.names:
            name = alias.name
            for (mod, msg, err) in IMPORT_RULES:
                if name == mod or name.startswith(mod + "."):
                    self._add(node, msg, err)
                    break
        self.generic_visit(node)

    def visit_ImportFrom(self, node):
        module = node.module or ""
        for (mod, msg, err) in IMPORT_RULES:
            if module == mod or module.startswith(mod + "."):
                self._add(node, msg, err)
                break
        self.generic_visit(node)

    def visit_Call(self, node):
        func = node.func
        if isinstance(func, ast.Attribute):
            # obj.method(...)
            obj_name = func.value.id if isinstance(func.value, ast.Name) else None
            if obj_name:
                for (obj, method, msg, err) in ATTR_CALL_RULES:
                    if obj_name == obj and func.attr == method:
                        self._add(node, msg, err)
                        break
        elif isinstance(func, ast.Name):
            # bare_name(...)
            for (name, msg, err) in NAME_CALL_RULES:
                if func.id == name:
                    self._add(node, msg, err)
                    break
        self.generic_visit(node)

# ── Main ──────────────────────────────────────────────────────────────────────

all_findings = []
for path in sys.argv[1:]:
    try:
        with open(path, encoding="utf-8") as fh:
            src = fh.read()
        lines = src.splitlines()
        tree = ast.parse(src, filename=path)
        checker = Checker(path, lines)
        checker.visit(tree)
        all_findings.extend(checker.findings)
    except SyntaxError as e:
        all_findings.append({
            "file": path, "line": e.lineno or 0,
            "message": "syntax error: " + (e.msg or str(e)),
            "is_error": True, "source": "",
        })
    except Exception as e:
        all_findings.append({
            "file": path, "line": 0,
            "message": "analysis failed: " + str(e),
            "is_error": False, "source": "",
        })

print(json.dumps(all_findings))
"#;

// ── Public interface ──────────────────────────────────────────────────────────

/// Scaffold a new Python plugin project named `name` in the current directory.
pub fn scaffold(name: &str) -> Result<()> {
    // ── 1. Choose framework ───────────────────────────────────────────────
    println!("\nChoose a Python MCP framework:");
    println!("  1) FastMCP      (async, ergonomic — requires mcp[server] + anyio)");
    println!("  2) Pure Python  (no runtime deps  — manual JSON-RPC over stdio)");
    let choice = prompt("\nEnter 1 or 2: ")?;
    let use_fastmcp = match choice.as_str() {
        "1" => true,
        "2" => false,
        other => bail!("invalid choice '{other}' — enter 1 or 2"),
    };

    // ── 2. Create project directory ───────────────────────────────────────
    let dir = std::path::Path::new(name);
    if dir.exists() {
        bail!("directory '{name}/' already exists — remove it or choose a different name");
    }
    std::fs::create_dir(dir)
        .with_context(|| format!("creating directory '{name}/'"))?;
    std::fs::create_dir(dir.join("wit"))
        .with_context(|| format!("creating '{name}/wit/'"))?;

    // ── 3. Write project files ────────────────────────────────────────────
    let entry = if use_fastmcp { WASM_ENTRY_FASTMCP } else { WASM_ENTRY_PURE };
    let app   = if use_fastmcp { APP_PY_FASTMCP }    else { APP_PY_PURE };
    let reqs  = if use_fastmcp { REQUIREMENTS_FASTMCP } else { REQUIREMENTS_PURE };
    write_file(dir.join("wasm_entry.py"),    entry)?;
    write_file(dir.join("app.py"),           fill(app, name))?;
    write_file(dir.join("requirements.txt"), reqs)?;
    write_file(dir.join("wit/world.wit"),    WIT_WORLD)?;
    write_file(dir.join("manifest.toml"),    fill(MANIFEST_TOML, name))?;
    write_file(dir.join(".gitignore"),       GITIGNORE)?;

    // ── 4. Print next steps ───────────────────────────────────────────────
    println!("\n✓  {name}/ created");
    println!("\nNext steps:");
    println!("  1. cd {name}");
    println!("  2. python3 -m venv .venv && source .venv/bin/activate");
    if use_fastmcp {
        println!("  3. pip install componentize-py mcp anyio");
    } else {
        println!("  3. pip install componentize-py");
    }
    println!("  4. craft mcp build");

    Ok(())
}

/// Static analysis — run the embedded AST analyser on all `.py` files in
/// `dir`, skipping `wasm_entry.py` (the shim is controlled by the scaffold).
pub fn analyse(dir: &Path) -> Result<Vec<Finding>> {
    let py_files: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()) == Some("py")
                && p.file_name().and_then(|n| n.to_str()) != Some("wasm_entry.py")
        })
        .collect();

    if py_files.is_empty() {
        return Ok(vec![]);
    }

    // Run: python3 -c <ANALYSER_PY> file1.py file2.py ...
    let python = find_python().context(
        "Python 3 not found — required for static analysis.\n\
         Install Python 3 and ensure 'python3' or 'python' is on your PATH.",
    )?;

    let out = Command::new(&python)
        .arg("-c")
        .arg(ANALYSER_PY)
        .args(&py_files)
        .output()
        .with_context(|| format!("running {python} analyser"))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("Python analyser failed:\n{stderr}");
    }

    // Parse JSON output
    let json_out = String::from_utf8_lossy(&out.stdout);
    let raw: Vec<serde_json::Value> = serde_json::from_str(&json_out)
        .context("parsing analyser output (expected JSON array)")?;

    let mut findings = Vec::new();
    for item in raw {
        findings.push(Finding {
            file:     item["file"].as_str().unwrap_or("").into(),
            line_no:  item["line"].as_u64().unwrap_or(0) as usize,
            line:     item["source"].as_str().unwrap_or("").to_string(),
            message:  item["message"].as_str().unwrap_or("").to_string(),
            is_error: item["is_error"].as_bool().unwrap_or(false),
        });
    }
    Ok(findings)
}

/// Build/compile the Python plugin in `dir` to `<name>.wasm`.
pub fn build(dir: &Path, name: &str) -> Result<()> {
    // ── Locate componentize-py ────────────────────────────────────────────
    println!("\nLocating componentize-py…");

    enum Mode { Direct, PythonModule, Uv }

    let mode = if cmd_exists("componentize-py") {
        println!("  Found: componentize-py");
        Mode::Direct
    } else if cmd_check("python3", &["-m", "componentize_py", "--version"]) {
        println!("  Found: python3 -m componentize_py");
        Mode::PythonModule
    } else if cmd_exists("uv") {
        println!("  Found: uv run componentize-py");
        Mode::Uv
    } else {
        bail!(
            "componentize-py not found.\n\
             Install it with:\n\
             \n\
               pip install componentize-py\n\
               # or: uv add componentize-py\n\
             \n\
             Then re-run `craft mcp build`."
        )
    };

    // ── Run componentize-py ───────────────────────────────────────────────
    let wasm_out = format!("{name}.wasm");
    println!("Compiling {name} → {wasm_out}…");

    let cp_args: &[&str] = &["-d", "wit", "-w", "mcp:plugin/plugin",
                               "componentize", "wasm_entry", "-o", &wasm_out];

    let status = match mode {
        Mode::Direct => {
            Command::new("componentize-py")
                .args(cp_args).current_dir(dir).status()
        }
        Mode::PythonModule => {
            Command::new("python3")
                .args(["-m", "componentize_py"])
                .args(cp_args).current_dir(dir).status()
        }
        Mode::Uv => {
            Command::new("uv")
                .args(["run", "componentize-py"])
                .args(cp_args).current_dir(dir).status()
        }
    }
    .context("launching componentize-py")?;

    if !status.success() {
        bail!("componentize-py exited with {status}\nCheck the output above for details.");
    }

    // ── Verify output ─────────────────────────────────────────────────────
    let wasm_path = dir.join(&wasm_out);
    if !wasm_path.exists() {
        bail!("compilation succeeded but {wasm_out} was not created — check componentize-py output");
    }
    let kb = (std::fs::metadata(&wasm_path).map(|m| m.len()).unwrap_or(0) + 1023) / 1024;

    // ── Optional wasm-tools verification ─────────────────────────────────
    if cmd_exists("wasm-tools") {
        println!("Verifying component with wasm-tools…");
        let ok = Command::new("wasm-tools")
            .args(["component", "wit", &wasm_out])
            .current_dir(dir)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            println!("  Component interface verified.");
        } else {
            eprintln!("  WARN: wasm-tools verification failed — the .wasm may be malformed");
        }
    }

    println!("\n✓  {wasm_out} ({kb} KB)");
    println!("   Install with: craft mcp install {}", wasm_path.display());
    Ok(())
}

// ── Private helpers ───────────────────────────────────────────────────────────

/// Return the name of the first available Python 3 interpreter.
fn find_python() -> Option<String> {
    for candidate in ["python3", "python"] {
        if cmd_check(candidate, &["--version"]) {
            return Some(candidate.to_string());
        }
    }
    None
}

fn cmd_exists(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn cmd_check(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
