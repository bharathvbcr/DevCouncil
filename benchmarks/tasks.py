"""Benchmark task suite.

Each task ships:
  - goal:        the terse natural-language prompt a user would actually type.
  - spec:        an elaborated prompt that spells out edge cases (arm C control).
  - seed:        {filename: contents} starting files for the workspace.
  - target_file: the module file the function lives in (loaded for scoring).
  - checks:      {check_name: python_expression} hidden ground-truth.

Hidden-check expressions run in a scoring scaffold (see run_bench.py) with the
target module bound to ``m`` and two helpers available:
  - raises(fn, *args, exc=Exception) -> bool   # fn(*args) raised exc
  - no_mut(fn, arg) -> bool                     # fn(arg) did not mutate arg
The agent never sees `checks` or `spec` for arm A; this file stays out of the
workspace entirely.
"""

from dataclasses import dataclass, field


@dataclass
class Task:
    name: str
    goal: str
    spec: str
    seed: dict
    target_file: str
    checks: dict = field(default_factory=dict)


_PY_HEADER = '"""Module under benchmark."""\n\n'


TASKS = [
    Task(
        name="median",
        goal="Add a median(values) function to stats.py that returns the median of a list of numbers.",
        spec=(
            "Add median(values) to stats.py. Return the middle value for odd-length "
            "input and the average of the two middle values (a float) for even-length "
            "input. Do not mutate the input list. Raise ValueError on empty input. "
            "Handle unsorted input."
        ),
        seed={"stats.py": _PY_HEADER + "def mean(values):\n    return sum(values) / len(values)\n"},
        target_file="stats.py",
        checks={
            "odd_value": "m.median([3, 1, 2]) == 2",
            "even_is_average_float": "m.median([1, 2, 3, 4]) == 2.5",
            "does_not_mutate_input": "no_mut(m.median, [3, 1, 2])",
            "empty_raises_valueerror": "raises(m.median, [], exc=ValueError)",
        },
    ),
    Task(
        name="chunk",
        goal="Add a chunk(items, size) function to lists.py that splits a list into sublists of length size.",
        spec=(
            "Add chunk(items, size) to lists.py returning a list of sublists each of "
            "length `size`, with the final chunk shorter if needed. Raise ValueError "
            "for size <= 0. Do not mutate the input. If size >= len(items), return a "
            "single chunk containing all items."
        ),
        seed={"lists.py": _PY_HEADER},
        target_file="lists.py",
        checks={
            "even_split": "m.chunk([1, 2, 3, 4], 2) == [[1, 2], [3, 4]]",
            "last_partial_chunk": "m.chunk([1, 2, 3, 4, 5], 2) == [[1, 2], [3, 4], [5]]",
            "size_zero_raises": "raises(m.chunk, [1, 2], 0, exc=ValueError)",
            "size_larger_than_list": "m.chunk([1, 2], 5) == [[1, 2]]",
            "does_not_mutate_input": "no_mut(lambda x: m.chunk(x, 2), [1, 2, 3])",
        },
    ),
    Task(
        name="roman",
        goal="Add a roman_to_int(s) function to roman.py that converts a Roman numeral string to an integer.",
        spec=(
            "Add roman_to_int(s) to roman.py. Support subtractive notation (IV=4, "
            "IX=9, XL=40, CM=900). Accept lowercase too. Raise ValueError for an "
            "empty string or a string containing non-Roman characters."
        ),
        seed={"roman.py": _PY_HEADER},
        target_file="roman.py",
        checks={
            "simple": "m.roman_to_int('III') == 3",
            "subtractive_iv": "m.roman_to_int('IV') == 4",
            "complex_1994": "m.roman_to_int('MCMXCIV') == 1994",
            "lowercase_accepted": "m.roman_to_int('xiv') == 14",
            "empty_raises": "raises(m.roman_to_int, '', exc=ValueError)",
            "invalid_char_raises": "raises(m.roman_to_int, 'IXZ', exc=ValueError)",
        },
    ),
    Task(
        name="balanced",
        goal="Add an is_balanced(s) function to brackets.py that returns True if the brackets in s are balanced.",
        spec=(
            "Add is_balanced(s) to brackets.py returning True iff (), [] and {} are "
            "correctly nested and matched. Ignore all non-bracket characters. An "
            "empty string is balanced. Mismatched types like '(]' are not balanced."
        ),
        seed={"brackets.py": _PY_HEADER},
        target_file="brackets.py",
        checks={
            "simple_pairs": "m.is_balanced('()[]{}') is True",
            "nested": "m.is_balanced('([{}])') is True",
            "wrong_type_not_balanced": "m.is_balanced('(]') is False",
            "interleaved_not_balanced": "m.is_balanced('([)]') is False",
            "empty_is_balanced": "m.is_balanced('') is True",
            "ignores_other_chars": "m.is_balanced('a(b)c') is True",
        },
    ),
    Task(
        name="parse_kv",
        goal="Add a parse_kv(text) function to config.py that parses 'key=value' lines into a dict.",
        spec=(
            "Add parse_kv(text) to config.py. Parse newline-separated 'key=value' "
            "lines into a dict. Skip blank lines and lines starting with '#'. Strip "
            "surrounding whitespace from keys and values. Split only on the first "
            "'=' (values may contain '='). On duplicate keys, the last one wins."
        ),
        seed={"config.py": _PY_HEADER},
        target_file="config.py",
        checks={
            "basic": "m.parse_kv('a=1\\nb=2') == {'a': '1', 'b': '2'}",
            "skips_blank_and_comments": "m.parse_kv('a=1\\n\\n# c=3\\nb=2') == {'a': '1', 'b': '2'}",
            "strips_whitespace": "m.parse_kv('  a =  1 ') == {'a': '1'}",
            "splits_on_first_equals": "m.parse_kv('url=http://x?a=b') == {'url': 'http://x?a=b'}",
            "duplicate_last_wins": "m.parse_kv('a=1\\na=2') == {'a': '2'}",
        },
    ),
    Task(
        name="merge_intervals",
        goal="Add a merge_intervals(intervals) function to intervals.py that merges overlapping intervals.",
        spec=(
            "Add merge_intervals(intervals) to intervals.py. Each interval is a "
            "[start, end] pair. Merge overlapping and adjacent intervals ([1,2] and "
            "[2,3] merge to [1,3]). Accept unsorted input. Return [] for empty input. "
            "Do not mutate the input."
        ),
        seed={"intervals.py": _PY_HEADER},
        target_file="intervals.py",
        checks={
            "overlapping": "m.merge_intervals([[1, 3], [2, 6], [8, 10]]) == [[1, 6], [8, 10]]",
            "adjacent_merge": "m.merge_intervals([[1, 2], [2, 3]]) == [[1, 3]]",
            "unsorted_input": "m.merge_intervals([[8, 10], [1, 3], [2, 6]]) == [[1, 6], [8, 10]]",
            "empty": "m.merge_intervals([]) == []",
            "does_not_mutate_input": "no_mut(m.merge_intervals, [[1, 3], [2, 6]])",
        },
    ),
]

TASKS_BY_NAME = {t.name: t for t in TASKS}
