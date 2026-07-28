SDK_VERSION = "0.1.0"
CAPABILITIES = (
    "sdk_version",
    "capabilities",
    "tool_registry",
)

_TOOLS = {}

import inspect
import types
import typing


def sdk_version():
    return SDK_VERSION


def capabilities():
    return CAPABILITIES


def _annotation_to_schema(annotation):
    if annotation is inspect._empty:
        return {"type": "string"}

    origin = typing.get_origin(annotation)
    args = typing.get_args(annotation)

    if annotation is str:
        return {"type": "string"}
    if annotation is int:
        return {"type": "integer"}
    if annotation is float:
        return {"type": "number"}
    if annotation is bool:
        return {"type": "boolean"}

    if origin in (list, typing.List):
        item_annotation = args[0] if args else str
        return {
            "type": "array",
            "items": _annotation_to_schema(item_annotation),
        }

    if origin in (dict, typing.Dict):
        value_annotation = args[1] if len(args) >= 2 else str
        return {
            "type": "object",
            "additionalProperties": _annotation_to_schema(value_annotation),
        }

    if origin in (typing.Union, types.UnionType):
        non_none = [arg for arg in args if arg is not type(None)]
        if len(non_none) == 1 and len(non_none) != len(args):
            return _annotation_to_schema(non_none[0])

    return {"type": "string"}


def _function_to_schema(fn):
    signature = inspect.signature(fn)
    properties = {}
    required = []

    for parameter in signature.parameters.values():
        if parameter.kind not in (
            inspect.Parameter.POSITIONAL_OR_KEYWORD,
            inspect.Parameter.KEYWORD_ONLY,
        ):
            raise TypeError(
                f"phi.tool does not support parameter kind {parameter.kind!s} for {fn.__name__}"
            )
        properties[parameter.name] = _annotation_to_schema(parameter.annotation)
        if parameter.default is inspect._empty:
            required.append(parameter.name)

    schema = {
        "type": "object",
        "properties": properties,
        "additionalProperties": False,
    }
    if required:
        schema["required"] = required
    return schema


def _description_for_tool(fn, explicit_description):
    if explicit_description is not None:
        return explicit_description
    return inspect.getdoc(fn) or ""


def _wrap_callable(fn, explicit_schema):
    if explicit_schema is not None:
        return explicit_schema, lambda arguments: fn(arguments)

    inspect.signature(fn).bind_partial()

    def invoke(arguments):
        if not isinstance(arguments, dict):
            raise TypeError(
                f"phi plugin tool {fn.__name__} expected object arguments, got {type(arguments).__name__}"
            )
        return fn(**arguments)

    return _function_to_schema(fn), invoke


def tool(_fn=None, *, name=None, description=None, input_schema=None):
    def decorator(fn):
        tool_name = name or fn.__name__
        schema, callable_ = _wrap_callable(fn, input_schema)
        _TOOLS[tool_name] = {
            "name": tool_name,
            "description": _description_for_tool(fn, description),
            "parameters": schema,
            "callable": callable_,
        }
        return fn

    if _fn is None:
        return decorator
    return decorator(_fn)


def _list_tools():
    return [
        {
            "name": tool["name"],
            "description": tool["description"],
            "parameters": tool["parameters"],
        }
        for tool in _TOOLS.values()
    ]


def _call_tool(name, arguments):
    entry = _TOOLS.get(name)
    if entry is None:
        raise ValueError(f"unknown phi plugin tool: {name}")
    return entry["callable"](arguments)
