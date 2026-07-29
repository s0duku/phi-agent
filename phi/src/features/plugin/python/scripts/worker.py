import contextlib
import io
import json
import pathlib
import sys
import traceback
import types
import json as _json

SDK_SOURCE = __PHI_SDK_SOURCE__


def install_phi_sdk():
    module = types.ModuleType("__PHI_MODULE_NAME__")
    exec(compile(SDK_SOURCE, "<phi-sdk>", "exec"), module.__dict__, module.__dict__)
    sys.modules["__PHI_MODULE_NAME__"] = module


install_phi_sdk()

phi_globals = {
    "__name__": "__phi_plugin_env__",
    "__builtins__": __builtins__,
}
phi_locals = phi_globals
plugins = []


def plugin_name(source: str) -> str:
    stem = pathlib.Path(source).stem
    return stem or "plugin"


def send(payload):
    sys.stdout.write(json.dumps(payload) + "\n")
    sys.stdout.flush()


@contextlib.contextmanager
def protect_plugin_io():
    buffer = io.StringIO()
    with contextlib.redirect_stdout(buffer):
        yield
    output = buffer.getvalue()
    if output:
        print(output, file=sys.stderr, end="")


def normalize_tool_output(value):
    return _json.dumps(
        {
            "tool_ok": True,
            "tool_error": None,
            "value": value,
        },
        ensure_ascii=False,
    )


def _traceback_entries(exc):
    entries = []
    for frame in traceback.extract_tb(exc.__traceback__):
        entries.append(
            {
                "path": frame.filename,
                "source": pathlib.Path(frame.filename).name,
                "line": frame.lineno,
                "function": frame.name,
            }
        )
    return entries


def _is_internal_traceback_path(path):
    return path in ("<phi-sdk>", "<string>") or path.startswith("<")


def _tool_exception_location(exc):
    plugin_entries = [
        entry
        for entry in _traceback_entries(exc)
        if not _is_internal_traceback_path(entry["path"])
    ]
    if not plugin_entries:
        return None
    entry = plugin_entries[-1]
    return {
        "source": entry["source"],
        "line": entry["line"],
        "function": entry["function"],
    }


def report_tool_exception(tool_name, exc):
    message = str(exc)
    print(
        f"phi plugin tool '{tool_name}' failed: {type(exc).__name__}: {message}",
        file=sys.stderr,
    )
    location = _tool_exception_location(exc)
    if location is not None:
        print(
            "  at "
            f"{location['source']}:{location['line']} in {location['function']}",
            file=sys.stderr,
        )


def normalize_tool_exception(exc):
    message = str(exc)
    payload = {
        "type": type(exc).__name__,
        "message": message,
    }
    location = _tool_exception_location(exc)
    if location is not None:
        payload["location"] = location
    return _json.dumps(
        {
            "tool_ok": False,
            "tool_error": message,
            "value": payload,
        },
        ensure_ascii=False,
    )


while True:
    line = sys.stdin.readline()
    if not line:
        break

    try:
        request = json.loads(line)
        kind = request.get("kind")

        if kind == "ping":
            send({"ok": True})
            continue

        if kind == "load_plugin":
            source = request["source"]
            code = request["code"]
            if not code.strip():
                raise ValueError(f"plugin '{source}' is empty")

            phi_globals["__file__"] = source
            with protect_plugin_io():
                exec(compile(code, source, "exec"), phi_globals, phi_locals)
            plugins.append({"source": source})
            send({"ok": True, "name": plugin_name(source)})
            continue

        if kind == "run_code":
            code = request["code"]
            buffer = io.StringIO()
            with contextlib.redirect_stdout(buffer):
                exec(compile(code, "<phi-py>", "exec"), phi_globals, phi_locals)
            send({"ok": True, "output": buffer.getvalue()})
            continue

        if kind == "list_tools":
            import phi

            send({"ok": True, "tools": phi._list_tools()})
            continue

        if kind == "call_tool":
            import phi

            try:
                output = phi._call_tool(request["name"], request["arguments"])
                output = normalize_tool_output(output)
            except Exception as exc:
                report_tool_exception(request["name"], exc)
                send({"ok": True, "output": normalize_tool_exception(exc)})
                continue
            send({"ok": True, "output": output})
            continue

        send({"ok": False, "error": f"unknown request kind: {kind}"})
    except Exception as exc:
        send(
            {
                "ok": False,
                "error": "".join(traceback.format_exception_only(type(exc), exc)).strip(),
            }
        )
