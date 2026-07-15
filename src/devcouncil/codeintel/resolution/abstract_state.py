"""Bounded abstract values for computed names, imports, and paths."""

from __future__ import annotations

import ast
import os
import re
from dataclasses import dataclass, field
from typing import Iterable, Mapping

MAX_STRINGS = 32
MAX_CALLABLES = 32
MAX_TYPES = 16


@dataclass(frozen=True)
class AbstractValue:
    scalars: frozenset[str | int | float | bool | None] = frozenset()
    callables: frozenset[str] = frozenset()
    types: frozenset[str] = frozenset()
    unknown: bool = False

    @classmethod
    def unknown_value(cls) -> "AbstractValue":
        return cls(unknown=True)

    @classmethod
    def scalar(cls, value: str | int | float | bool | None) -> "AbstractValue":
        return cls(scalars=frozenset({value}))

    @classmethod
    def callable(cls, name: str) -> "AbstractValue":
        return cls(callables=frozenset({name}))

    @classmethod
    def type_name(cls, name: str) -> "AbstractValue":
        return cls(types=frozenset({name}))

    def merge(self, other: "AbstractValue") -> "AbstractValue":
        scalars = self.scalars | other.scalars
        callables = self.callables | other.callables
        types = self.types | other.types
        overflow = len(scalars) > MAX_STRINGS or len(callables) > MAX_CALLABLES or len(types) > MAX_TYPES
        if overflow:
            return AbstractValue.unknown_value()
        return AbstractValue(scalars, callables, types, self.unknown or other.unknown)

    def strings(self) -> frozenset[str]:
        return frozenset(value for value in self.scalars if isinstance(value, str))


@dataclass
class AbstractState:
    values: dict[str, AbstractValue] = field(default_factory=dict)
    functions: dict[str, tuple[tuple[str, ...], str]] = field(default_factory=dict)

    def assign(self, name: str, value: AbstractValue) -> None:
        self.values[name] = value

    def get(self, name: str) -> AbstractValue:
        return self.values.get(name, AbstractValue.unknown_value())

    def analyze(
        self,
        source: str,
        *,
        callable_names: Iterable[str] = (),
        type_names: Iterable[str] = (),
        callable_aliases: Mapping[str, str] | None = None,
        type_aliases: Mapping[str, str] | None = None,
    ) -> "AbstractState":
        """Collect bounded constants, aliases, object properties, and wrappers."""
        for name in callable_names:
            self.assign(name, AbstractValue.callable(name))
        for name in type_names:
            self.assign(name, AbstractValue.type_name(name))
        for local, remote in (callable_aliases or {}).items():
            self.assign(local, AbstractValue.callable(remote))
        for local, remote in (type_aliases or {}).items():
            self.assign(local, AbstractValue.type_name(remote))

        for match in re.finditer(
            r"^\s*function\s+([A-Za-z_$][\w$]*)\s*\(([^)]*)\)\s*"
            r"\{\s*return\s+(.+)\s*;\s*\}\s*$",
            source,
            re.MULTILINE,
        ):
            params = tuple(
                value.strip()
                for value in match.group(2).split(",")
                if value.strip()
            )
            self.functions[match.group(1)] = (params, match.group(3).rstrip("; "))

        for line in source.splitlines():
            assignment_match = re.match(
                r"^\s*(?:(?:const|let|var)\s+)?"
                r"([A-Za-z_$][\w$]*(?:\.[A-Za-z_$][\w$]*)?)\s*=\s*(.+?)\s*;?\s*$",
                line,
            )
            if assignment_match is None or "==" in line or "=>" in line:
                continue
            name, expression = assignment_match.groups()
            expression = expression.rstrip("; ")
            if expression.startswith("{") and expression.endswith("}"):
                for key, value in re.findall(
                    r"([A-Za-z_$][\w$]*)\s*:\s*([^,}]+)", expression
                ):
                    self.assign(f"{name}.{key}", self.evaluate(value.strip()))
                continue
            self.assign(name, self.evaluate(expression))
        return self

    def evaluate(self, expression: str) -> AbstractValue:
        """Evaluate Python or JS-like scalar/callable/type expressions."""
        expression = expression.strip().rstrip(";")
        python_value = self.evaluate_python(expression)
        if not python_value.unknown:
            return python_value
        return self._eval_text(expression)

    def evaluate_python(self, expression: str | ast.AST) -> AbstractValue:
        try:
            node = ast.parse(expression, mode="eval").body if isinstance(expression, str) else expression
        except SyntaxError:
            return AbstractValue.unknown_value()
        return self._eval(node)

    def _eval(self, node: ast.AST) -> AbstractValue:
        if isinstance(node, ast.Constant) and isinstance(node.value, (str, int, float, bool, type(None))):
            return AbstractValue.scalar(node.value)
        if isinstance(node, ast.Name):
            return self.get(node.id)
        if isinstance(node, ast.IfExp):
            return self._eval(node.body).merge(self._eval(node.orelse))
        if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Add):
            return self._combine_strings(self._eval(node.left).strings(), self._eval(node.right).strings(), "")
        if isinstance(node, ast.JoinedStr):
            choices: set[str] = {""}
            for part in node.values:
                if isinstance(part, ast.Constant) and isinstance(part.value, str):
                    values = {part.value}
                elif isinstance(part, ast.FormattedValue):
                    values = set(self._eval(part.value).strings())
                else:
                    return AbstractValue.unknown_value()
                choices = {left + right for left in choices for right in values}
                if len(choices) > MAX_STRINGS:
                    return AbstractValue.unknown_value()
            return AbstractValue(scalars=frozenset(choices))
        if isinstance(node, (ast.Tuple, ast.List, ast.Set)):
            value = AbstractValue()
            for item in node.elts:
                value = value.merge(self._eval(item))
            return value
        if isinstance(node, ast.Call):
            name = self._call_name(node.func)
            if name in {"str", "os.fspath"} and node.args:
                return self._eval(node.args[0])
            if name in {"os.path.join", "pathlib.Path", "Path"}:
                parts = [self._eval(arg).strings() for arg in node.args]
                if not parts or any(not part for part in parts):
                    return AbstractValue.unknown_value()
                path_values: set[str] = {""}
                for path_part in parts:
                    path_values = {
                        os.path.join(left, right) if left else right
                        for left in path_values
                        for right in path_part
                    }
                    if len(path_values) > MAX_STRINGS:
                        return AbstractValue.unknown_value()
                return AbstractValue(scalars=frozenset(value.replace("\\", "/") for value in path_values))
        return AbstractValue.unknown_value()

    def _eval_text(self, expression: str) -> AbstractValue:
        expression = expression.strip().rstrip(";")
        while expression.startswith("(") and expression.endswith(")"):
            expression = expression[1:-1].strip()
        if not expression:
            return AbstractValue.unknown_value()
        if re.fullmatch(r"[A-Za-z_$][\w$]*(?:\.[A-Za-z_$][\w$]*)*", expression):
            return self.get(expression)
        if (
            len(expression) >= 2
            and expression[0] == expression[-1]
            and expression[0] in {"'", '"'}
        ):
            try:
                return AbstractValue.scalar(ast.literal_eval(expression))
            except (SyntaxError, ValueError):
                return AbstractValue.unknown_value()
        if expression.startswith("`") and expression.endswith("`"):
            values = {expression[1:-1]}
            for nested in re.findall(r"\$\{([^}]+)\}", expression):
                replacements = self.evaluate(nested).strings()
                if not replacements:
                    return AbstractValue.unknown_value()
                values = {
                    value.replace(f"${{{nested}}}", replacement)
                    for value in values
                    for replacement in replacements
                }
                if len(values) > MAX_STRINGS:
                    return AbstractValue.unknown_value()
            return AbstractValue(scalars=frozenset(values))
        conditional = re.match(r"^.+?\?\s*(.+?)\s*:\s*(.+)$", expression)
        if conditional is not None:
            return self.evaluate(conditional.group(1)).merge(
                self.evaluate(conditional.group(2))
            )
        parts = self._split_top_level(expression, "+")
        if len(parts) > 1:
            value = self.evaluate(parts[0])
            for part in parts[1:]:
                value = self._combine_strings(
                    value.strings(), self.evaluate(part).strings(), ""
                )
            return value
        if expression.startswith("[") and expression.endswith("]"):
            value = AbstractValue()
            for part in self._split_top_level(expression[1:-1], ","):
                value = value.merge(self.evaluate(part))
            return value
        call = re.match(r"^([A-Za-z_$][\w$]*(?:\.[A-Za-z_$][\w$]*)*)\((.*)\)$", expression)
        if call is not None:
            name, raw_args = call.groups()
            args = [self.evaluate(part) for part in self._split_top_level(raw_args, ",")]
            if name in {"path.join", "path.resolve", "os.path.join", "Path", "pathlib.Path"}:
                choices = [arg.strings() for arg in args]
                if not choices or any(not choice for choice in choices):
                    return AbstractValue.unknown_value()
                values = {""}
                for choice in choices:
                    values = {
                        os.path.join(left, right) if left else right
                        for left in values
                        for right in choice
                    }
                    if len(values) > MAX_STRINGS:
                        return AbstractValue.unknown_value()
                return AbstractValue(
                    scalars=frozenset(value.replace("\\", "/") for value in values)
                )
            bound = self.get(name)
            if bound.types or bound.callables:
                return bound
            wrapper = self.functions.get(name)
            if wrapper is not None:
                params, body = wrapper
                previous = {param: self.values.get(param) for param in params}
                try:
                    for param, value in zip(params, args):
                        self.assign(param, value)
                    return self.evaluate(body)
                finally:
                    for param, previous_value in previous.items():
                        if previous_value is None:
                            self.values.pop(param, None)
                        else:
                            self.values[param] = previous_value
        return AbstractValue.unknown_value()

    @staticmethod
    def _split_top_level(expression: str, separator: str) -> list[str]:
        parts: list[str] = []
        start = 0
        depth = 0
        quote = ""
        for index, char in enumerate(expression):
            if quote:
                if char == quote and (index == 0 or expression[index - 1] != "\\"):
                    quote = ""
                continue
            if char in {"'", '"', "`"}:
                quote = char
            elif char in "([{":
                depth += 1
            elif char in ")]}":
                depth = max(0, depth - 1)
            elif char == separator and depth == 0:
                parts.append(expression[start:index].strip())
                start = index + 1
        parts.append(expression[start:].strip())
        return [part for part in parts if part]

    @staticmethod
    def _call_name(node: ast.AST) -> str:
        if isinstance(node, ast.Name):
            return node.id
        if isinstance(node, ast.Attribute):
            prefix = AbstractState._call_name(node.value)
            return f"{prefix}.{node.attr}" if prefix else node.attr
        return ""

    @staticmethod
    def _combine_strings(left: Iterable[str], right: Iterable[str], separator: str) -> AbstractValue:
        values = {f"{a}{separator}{b}" for a in left for b in right}
        if not values or len(values) > MAX_STRINGS:
            return AbstractValue.unknown_value()
        return AbstractValue(scalars=frozenset(values))
