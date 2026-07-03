"""Self-check: score reference implementations (must be full) and a buggy one
(must be lower). Validates the hidden ground-truth + scoring scaffold offline.
Run: python benchmarks/_validate.py
"""
import sys
import tempfile
import shutil
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from tasks import TASKS_BY_NAME
from run_bench import score

REFERENCE = {
    "median": '''
def mean(values):
    return sum(values) / len(values)
def median(values):
    if not values:
        raise ValueError("empty")
    s = sorted(values); n = len(s); mid = n // 2
    return s[mid] if n % 2 else (s[mid - 1] + s[mid]) / 2
''',
    "chunk": '''
def chunk(items, size):
    if size <= 0:
        raise ValueError("size must be positive")
    return [items[i:i + size] for i in range(0, len(items), size)]
''',
    "roman": '''
def roman_to_int(s):
    if not s:
        raise ValueError("empty")
    s = s.upper()
    vals = {"I":1,"V":5,"X":10,"L":50,"C":100,"D":500,"M":1000}
    if any(ch not in vals for ch in s):
        raise ValueError("bad char")
    total = 0
    for i, ch in enumerate(s):
        v = vals[ch]
        if i + 1 < len(s) and vals[s[i + 1]] > v:
            total -= v
        else:
            total += v
    return total
''',
    "balanced": '''
def is_balanced(s):
    pairs = {")":"(", "]":"[", "}":"{"}
    stack = []
    for ch in s:
        if ch in "([{":
            stack.append(ch)
        elif ch in ")]}":
            if not stack or stack.pop() != pairs[ch]:
                return False
    return not stack
''',
    "parse_kv": '''
def parse_kv(text):
    out = {}
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        k, v = line.split("=", 1)
        out[k.strip()] = v.strip()
    return out
''',
    "merge_intervals": '''
def merge_intervals(intervals):
    s = sorted(list(i) for i in intervals)
    out = []
    for iv in s:
        if out and iv[0] <= out[-1][1]:
            out[-1][1] = max(out[-1][1], iv[1])
        else:
            out.append(list(iv))
    return out
''',
    "eval_rpn": '''
def eval_rpn(tokens):
    stack = []
    ops = {"+", "-", "*", "/"}
    for tok in tokens:
        if tok in ops:
            if len(stack) < 2:
                raise ValueError("too few operands")
            b = stack.pop(); a = stack.pop()
            if tok == "+": r = a + b
            elif tok == "-": r = a - b
            elif tok == "*": r = a * b
            else: r = int(a / b)  # truncates toward zero; ZeroDivisionError propagates
            stack.append(r)
        else:
            stack.append(int(tok))
    if len(stack) != 1:
        raise ValueError("leftover operands")
    return stack[0]
''',
    "base_convert": '''
def base_convert(number, from_base, to_base):
    if not (2 <= from_base <= 36) or not (2 <= to_base <= 36):
        raise ValueError("base out of range")
    digits = "0123456789abcdefghijklmnopqrstuvwxyz"
    s = str(number).strip().lower()
    if not s:
        raise ValueError("empty")
    value = 0
    for ch in s:
        d = digits.find(ch)
        if d == -1 or d >= from_base:
            raise ValueError("invalid digit")
        value = value * from_base + d
    if value == 0:
        return "0"
    out = []
    while value:
        out.append(digits[value % to_base])
        value //= to_base
    return "".join(reversed(out))
''',
    "parse_csv_line": '''
def parse_csv_line(line):
    fields, cur, in_quotes = [], [], False
    i = 0
    while i < len(line):
        ch = line[i]
        if in_quotes:
            if ch == '"':
                if i + 1 < len(line) and line[i + 1] == '"':
                    cur.append('"'); i += 1
                else:
                    in_quotes = False
            else:
                cur.append(ch)
        elif ch == '"':
            in_quotes = True
        elif ch == ",":
            fields.append("".join(cur)); cur = []
        else:
            cur.append(ch)
        i += 1
    fields.append("".join(cur))
    return fields
''',
    "flatten": '''
def flatten(nested):
    out = []
    for item in nested:
        if isinstance(item, list):
            out.extend(flatten(item))
        else:
            out.append(item)
    return out
''',
    "parse_duration": '''
import re
def parse_duration(s):
    compact = re.sub(r"\\s+", "", s)
    if not compact:
        raise ValueError("empty")
    matches = re.findall(r"(\\d+)([a-zA-Z])", compact)
    if "".join(n + u for n, u in matches) != compact:
        raise ValueError("bad format")
    units = {"d": 86400, "h": 3600, "m": 60, "s": 1}
    total = 0
    for num, unit in matches:
        u = unit.lower()
        if u not in units:
            raise ValueError("unknown unit")
        total += int(num) * units[u]
    return total
''',
}

# A buggy median: happy path only, no empty-input handling, integer even-average.
BUGGY_MEDIAN = '''
def mean(values):
    return sum(values) / len(values)
def median(values):
    values.sort()            # mutates input!
    n = len(values); mid = n // 2
    if n % 2:
        return values[mid]
    return (values[mid - 1] + values[mid]) // 2   # integer division
'''


def main():
    base = Path(tempfile.mkdtemp(prefix="dc_bench_validate_"))
    ok = True
    print("Reference implementations (expect full score):")
    for name, task in TASKS_BY_NAME.items():
        ws = base / name
        ws.mkdir(parents=True)
        (ws / task.target_file).write_text(REFERENCE[name], encoding="utf-8")
        r = score(ws, task, sys.executable)
        full = r["passed"] == r["total"]
        ok = ok and full
        flag = "OK " if full else "!! "
        print(f"  {flag}{name:16} {r['passed']}/{r['total']}  {r['detail']}")

    print("\nBuggy median (expect < full — traps must catch it):")
    task = TASKS_BY_NAME["median"]
    ws = base / "buggy"
    ws.mkdir(parents=True)
    (ws / task.target_file).write_text(BUGGY_MEDIAN, encoding="utf-8")
    r = score(ws, task, sys.executable)
    discriminates = r["passed"] < r["total"]
    ok = ok and discriminates
    print(f"  {'OK ' if discriminates else '!! '}buggy median     {r['passed']}/{r['total']}  {r['detail']}")

    shutil.rmtree(base, ignore_errors=True)
    print("\nVALIDATION:", "PASS" if ok else "FAIL")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
