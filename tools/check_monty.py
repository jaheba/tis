#!/usr/bin/env python3
"""Audit Monty's fixture expectations using real tis processes, without executing fixtures on the host."""

from __future__ import annotations

import argparse
import ast
import collections
import ctypes
import hashlib
import json
import os
import signal
import struct
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import tomllib

HOST_MODULES = {"os", "pathlib", "posixpath", "sys", "time", "asyncio", "gc"}


def header(source):
    lines = source.splitlines()
    if "# /// monty" not in lines:
        return {}
    start = lines.index("# /// monty")
    end = lines.index("# ///", start + 1)
    return tomllib.loads(
        "\n".join((line[2:] if line.startswith("# ") else line[1:]) for line in lines[start + 1 : end])
    )


def prepare(source, name):
    config = header(source)
    if config.get("call-external") or config.get("mount-fs"):
        return None, {"status": "outside_scope", "reason": "host callbacks or mounts", "header": config}
    if config.get("run-async"):
        return None, {"status": "outside_scope", "reason": "async execution", "header": config}
    if "monty" in config.get("xfail", []):
        return None, {"status": "outside_target", "reason": "fixture expects Monty failure", "header": config}
    if "cpython" in config.get("xfail", []):
        return None, {
            "status": "intentional_divergence",
            "reason": "fixture expects CPython failure",
            "header": config,
        }
    lines = source.rstrip().splitlines()
    if lines and lines[-1].startswith("# ref-counts="):
        return None, {"status": "outside_scope", "reason": "reference-count expectations", "header": config}
    expectation = "no_exception"
    expected = None
    for prefix, kind in [
        ("# Return.str=", "str"),
        ("# Return.type=", "type"),
        ("# Return=", "repr"),
        ("# Raise=", "raise"),
    ]:
        if lines and lines[-1].startswith(prefix):
            expectation = kind
            expected = lines[-1][len(prefix) :]
            lines.pop()
            break
    code = "\n".join(lines) + "\n"
    marker = "__TIS_COMPAT_" + hashlib.sha256(source.encode()).hexdigest()[:16] + "__"
    info = {"header": config, "expectation": expectation, "expected": expected, "marker": marker}
    try:
        tree = ast.parse(code, filename=name)
    except SyntaxError:
        info["syntax_fixture"] = True
        return code, info
    imports = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            imports.update(a.name.split(".")[0] for a in node.names)
        if isinstance(node, ast.ImportFrom):
            imports.add((node.module or "").split(".")[0])
    info["imports"] = sorted(imports)
    if imports & HOST_MODULES:
        return None, {
            **info,
            "status": "outside_scope",
            "reason": "host/runtime module: " + ", ".join(sorted(imports & HOST_MODULES)),
        }
    if any(
        isinstance(n, (ast.AsyncFunctionDef, ast.Await, ast.AsyncFor, ast.AsyncWith)) for n in ast.walk(tree)
    ):
        return None, {**info, "status": "outside_scope", "reason": "async language"}
    if any(
        isinstance(n, ast.Call) and isinstance(n.func, ast.Name) and n.func.id in {"open", "__import__"}
        for n in ast.walk(tree)
    ):
        return None, {**info, "status": "outside_scope", "reason": "host I/O or dynamic imports"}
    if (
        tree.body
        and isinstance(tree.body[-1], ast.Expr)
        and isinstance(tree.body[-1].value, ast.Constant)
        and isinstance(tree.body[-1].value.value, str)
        and tree.body[-1].value.value.lstrip().startswith("TRACEBACK:")
    ):
        node = tree.body.pop()
        info["expectation"] = "traceback"
        info["expected"] = node.value.value
        raw = code.encode()
        offset = (
            sum(len(l.encode()) for l in code.splitlines(keepends=True)[: node.lineno - 1]) + node.col_offset
        )
        code = raw[:offset].decode()
        expectation = "traceback"
    if expectation in {"repr", "str", "type"}:
        if not tree.body or not isinstance(tree.body[-1], ast.Expr):
            raise ValueError("return expectation without final expression")
        node = tree.body[-1]
        expr = ast.get_source_segment(code, node.value)
        value = f"type(({expr})).__name__" if expectation == "type" else f"{expectation}(({expr}))"
        before = code.splitlines(keepends=True)
        start = sum(len(l.encode()) for l in before[: node.lineno - 1]) + node.col_offset
        end = sum(len(l.encode()) for l in before[: node.end_lineno - 1]) + node.end_col_offset
        raw = code.encode()
        code = (raw[:start] + f'print({marker!r} + "return:" + {value})'.encode() + raw[end:]).decode()
    else:
        code += f'\nprint({marker!r} + "ok")\n'
    # Keep source indentation and multiline literals unchanged. Only the final expression
    # gains an observer; exceptions are read from the CLI's own error output.
    return code, info


def limits(memory_mib):
    import resource

    os.setsid()
    if sys.platform != "darwin":
        resource.setrlimit(resource.RLIMIT_AS, (memory_mib * 1024**2,) * 2)
    resource.setrlimit(resource.RLIMIT_FSIZE, (2 * 1024**2,) * 2)


def resident_bytes(pid):
    # macOS reserves a huge shared virtual region and cannot use a small RLIMIT_AS.
    # Poll only our own child; this is a test-driver budget, not a sandbox guarantee.
    if sys.platform != "darwin":
        return 0
    buf = ctypes.create_string_buffer(1024)
    lib = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
    size = lib.proc_pidinfo(pid, 4, 0, buf, len(buf))
    return struct.unpack("QQ", buf.raw[:16])[1] if size >= 16 else 0


def expected_error(expected):
    try:
        call = ast.parse(expected, mode="eval").body
        if isinstance(call, ast.Call):
            kind = ast.unparse(call.func)
            args = [ast.literal_eval(a) for a in call.args]
            message = str(args[0]) if len(args) == 1 else (str(tuple(args)) if args else "")
            return kind, message
    except (SyntaxError, ValueError, TypeError):
        pass
    return str(expected).split("(")[0], None


def classify(info, returncode, stdout, stderr):
    records = [line.split(info["marker"], 1)[1] for line in stdout.splitlines() if info["marker"] in line]
    value = records[-1] if records else None
    if returncode < 0:
        return {"status": "crash", "signal": -returncode}
    if returncode != 0:
        text = stderr.strip()
        if text.startswith("unsupported:"):
            return {"status": "unsupported", "reason": text}
        if text.startswith("invalid IR:") or text.startswith("object does not belong"):
            return {"status": "engine_error", "reason": text}
        mapping = {
            "type error:": "TypeError",
            "syntax error:": "SyntaxError",
            "index out of range": "IndexError",
            "key not found": "KeyError",
            "integer overflow": "OverflowError",
            "expected ": "TypeError",
        }
        actual_type = None
        message = text
        for prefix, kind in mapping.items():
            if text.startswith(prefix):
                actual_type = kind
                message = text[len(prefix) :].lstrip() if prefix.endswith(":") else text
                break
        if actual_type is None:
            prefix, _, rest = text.partition(":")
            if prefix.isidentifier() and (
                prefix.endswith("Error")
                or prefix.endswith("Exception")
                or prefix in {"StopIteration", "Exception", "BaseException"}
            ):
                actual_type = prefix
                message = rest.strip()
        if actual_type is None:
            return {"status": "engine_error", "reason": text}
        if info["expectation"] == "raise":
            expected_type, expected_message = expected_error(info["expected"])
            if actual_type == expected_type:
                return {
                    "status": "pass" if message == expected_message else "exception_message_difference",
                    "exception_type": actual_type,
                    "message": message,
                    "expected_message": expected_message,
                }
        if info["expectation"] == "traceback":
            last = info["expected"].strip().splitlines()[-1]
            expected_type = last.split(":")[0]
            return {
                "status": "traceback_unverified"
                if actual_type == expected_type
                else "traceback_type_difference",
                "exception_type": actual_type,
                "expected_type": expected_type,
                "message": message,
            }
        return {"status": "exception", "exception_type": actual_type, "message": message}
    if value is None:
        return {"status": "harness_unverified", "reason": "no observation marker"}
    if info["expectation"] == "raise" or info["expectation"] == "traceback":
        return {"status": "missing_exception", "actual": value}
    if info["expectation"] in {"repr", "str", "type"}:
        return {
            "status": "pass" if value == "return:" + info["expected"] else "value_difference",
            "actual": value.removeprefix("return:"),
        }
    return {"status": "pass", "coverage": "assertions" if info.get("assertions") else "execution_only"}


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--monty", type=Path, required=True)
    ap.add_argument("--tis", type=Path, required=True)
    ap.add_argument("--tier", choices=["bytecode", "native", "lir", "copy-patch"], default="bytecode")
    ap.add_argument("--output", type=Path, required=True)
    ap.add_argument("--timeout", type=float, default=3)
    ap.add_argument("--memory-mib", type=int, default=768)
    ap.add_argument("--filter", default="")
    args = ap.parse_args()
    if args.timeout <= 0 or args.memory_mib <= 0:
        ap.error("timeout and memory budget must be positive")
    if os.name != "posix":
        ap.error("fixture isolation currently requires a POSIX host")
    corpus = args.monty / "crates/monty/test_cases"
    files = sorted(corpus.glob("*.py"))
    if not files:
        ap.error(f"no fixtures found in {corpus}")
    args.output.mkdir(parents=True, exist_ok=False)
    out = {
        "monty_revision": subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=args.monty, text=True
        ).strip(),
        "tis_sha256": hashlib.sha256(args.tis.read_bytes()).hexdigest(),
        "tier": args.tier,
        "timeout_seconds": args.timeout,
        "memory_mib": args.memory_mib,
        "memory_budget_mode": "10ms RSS polling on macOS; RLIMIT_AS on other POSIX hosts",
        "reference": "Monty fixture expectations; no host execution or CPython stdout comparison",
        "cases": {},
    }
    subprocess.run([str(args.tis), "--version"], check=True, capture_output=True)
    with tempfile.TemporaryDirectory(prefix="tis-monty-compat-") as temp:
        for path in files:
            if args.filter and args.filter not in path.name:
                continue
            source = path.read_text()
            item = {
                "source_sha256": hashlib.sha256(source.encode()).hexdigest(),
                "category": path.stem.split("__")[0],
            }
            try:
                code, info = prepare(source, path.name)
            except Exception as e:
                out["cases"][path.name] = {**item, "status": "fixture_error", "reason": str(e)}
                continue
            item.update(info)
            if code is not None:
                try:
                    item["assertions"] = sum(isinstance(n, ast.Assert) for n in ast.walk(ast.parse(source)))
                except SyntaxError:
                    item["assertions"] = 0
                test = Path(temp) / path.name
                test.write_text(code)
                start = time.perf_counter_ns()
                with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
                    process = subprocess.Popen(
                        [str(args.tis), "--tier", args.tier, str(test)],
                        stdout=stdout,
                        stderr=stderr,
                        preexec_fn=(lambda: limits(args.memory_mib)) if os.name == "posix" else None,
                    )
                    deadline = time.monotonic() + args.timeout
                    peak = 0
                    while process.poll() is None:
                        peak = max(peak, resident_bytes(process.pid))
                        if peak > args.memory_mib * 1024**2 or time.monotonic() > deadline:
                            item["status"] = (
                                "memory_budget" if peak > args.memory_mib * 1024**2 else "timeout"
                            )
                            if os.name == "posix":
                                os.killpg(process.pid, signal.SIGKILL)
                            else:
                                process.kill()
                            process.wait()
                            break
                        time.sleep(0.01)
                    item["observed_peak_rss"] = peak
                    stdout.seek(0)
                    stderr.seek(0)
                    text = stdout.read(2 * 1024**2).decode(errors="replace")
                    err = stderr.read(2 * 1024**2).decode(errors="replace")
                    if item.get("status") not in {"timeout", "memory_budget"}:
                        item.update(classify(item, process.returncode, text, err))
                    item.update(
                        returncode=process.returncode,
                        elapsed_ns=time.perf_counter_ns() - start,
                        stdout=text,
                        stderr=err,
                    )
            out["cases"][path.name] = item
            if len(out["cases"]) % 50 == 0:
                print(
                    len(out["cases"]),
                    dict(collections.Counter(x["status"] for x in out["cases"].values())),
                    flush=True,
                )
            (args.output / "results.json").write_text(json.dumps(out, indent=2) + "\n")
    summary = collections.Counter(x["status"] for x in out["cases"].values())
    categories = {}
    for x in out["cases"].values():
        categories.setdefault(x["category"], collections.Counter())[x["status"]] += 1
    out["summary"] = dict(summary)
    out["categories"] = {k: dict(v) for k, v in sorted(categories.items())}
    (args.output / "results.json").write_text(json.dumps(out, indent=2) + "\n")
    print("Final", dict(summary))
    lines = [
        "# Monty fixture compatibility audit",
        "",
        f"Monty revision: {out['monty_revision']}. Tier: {args.tier}.",
        out["reference"] + ".",
        "",
        "Every fixture remains in the inventory. Outside-scope/target cases are not passes. Execution-only passes do not establish matching stdout or final values. Exception message differences and tracebacks remain separate.",
        "",
        "| Category | Status counts |",
        "|---|---|",
    ]
    lines.extend(
        f"| {k} | " + ", ".join(f"{s}: {n}" for s, n in sorted(v.items())) + " |"
        for k, v in out["categories"].items()
    )
    (args.output / "README.md").write_text("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
