"""Topology-E: Clone Matrix.

Algorithmic implementations and code fragments:
- 20 Type-1 exact duplicate clusters (token hash exact match).
- 20 Type-2 structural duplicate clusters (renamed variables, identical AST shape).
- 50 trivial getters, setters, and one-line delegators with AST node count < 32.
Asserts:
- Type-1 and Type-2 clones achieve 100% recall.
- All 50 trivial accessors are suppressed by MIN_SIGNATURE_NODES = 32 (0.0 noise floor).
"""

import hashlib
from typing import Dict, List, Set, Tuple

MIN_SIGNATURE_NODES = 32


class CodeFragment:
    def __init__(self, name: str, tokens: List[str], ast_shape: str, node_count: int, clone_type: str = None):
        self.name = name
        self.tokens = tokens
        self.ast_shape = ast_shape
        self.node_count = node_count
        self.clone_type = clone_type  # "T1", "T2", "trivial", "unique"


class TopologyEClones:
    def __init__(self):
        self.fragments: List[CodeFragment] = []
        self.ground_truth_t1: Set[Tuple[str, str]] = set()
        self.ground_truth_t2: Set[Tuple[str, str]] = set()
        self.ground_truth_trivial: Set[str] = set()
        self.build()

    def build(self):
        # 1. 20 Type-1 exact clone pairs (identical tokens and AST)
        for i in range(20):
            orig_name = f"t1_orig_{i}"
            clone_name = f"t1_clone_{i}"
            tokens = ["fn", "sort", "(", "arr", ")", "{", "for", "i", "in", "arr", "{", "swap", "}", "}"]
            ast_shape = "FunctionDef(For(Call(swap)))"
            node_count = 64  # Well above 32

            f1 = CodeFragment(orig_name, tokens, ast_shape, node_count, "T1")
            f2 = CodeFragment(clone_name, list(tokens), ast_shape, node_count, "T1")
            self.fragments.extend([f1, f2])
            self.ground_truth_t1.add((orig_name, clone_name))

        # 2. 20 Type-2 structural clone pairs (renamed identifiers, identical AST structure)
        for i in range(20):
            orig_name = f"t2_orig_{i}"
            clone_name = f"t2_clone_{i}"
            tokens_orig = ["fn", "transform_data", "(", "input_vec", ")", "{", "let", "res", "=", "map", "(", "input_vec", ")", ";", "return", "res", "}"]
            tokens_clone = ["fn", "process_items", "(", "raw_records", ")", "{", "let", "out", "=", "map", "(", "raw_records", ")", ";", "return", "out", "}"]
            ast_shape = "FunctionDef(Let(Call(map)),Return)"
            node_count = 48  # Above 32

            f1 = CodeFragment(orig_name, tokens_orig, ast_shape, node_count, "T2")
            f2 = CodeFragment(clone_name, tokens_clone, ast_shape, node_count, "T2")
            self.fragments.extend([f1, f2])
            self.ground_truth_t2.add((orig_name, clone_name))

        # 3. 50 trivial getters, setters, and accessors (node count < 32)
        for i in range(50):
            name = f"trivial_accessor_{i}"
            tokens = ["fn", f"get_field_{i}", "()", "{", "return", "self", ".", "x", "}"]
            ast_shape = "GetField"
            node_count = 8  # < 32!
            f = CodeFragment(name, tokens, ast_shape, node_count, "trivial")
            self.fragments.append(f)
            self.ground_truth_trivial.add(name)

    def detect_clones(self) -> Tuple[Set[Tuple[str, str]], Set[Tuple[str, str]], Set[str]]:
        """Simulates DevMap clone detection engine:
        - Filters out fragments with node_count < MIN_SIGNATURE_NODES (32).
        - Type-1: identical token hash.
        - Type-2: identical AST shape.
        """
        eligible = [f for f in self.fragments if f.node_count >= MIN_SIGNATURE_NODES]
        suppressed_trivial = {f.name for f in self.fragments if f.node_count < MIN_SIGNATURE_NODES}

        detected_t1 = set()
        detected_t2 = set()

        for i in range(len(eligible)):
            for j in range(i + 1, len(eligible)):
                f1 = eligible[i]
                f2 = eligible[j]

                # Type-1: exact token stream equality
                if f1.tokens == f2.tokens:
                    detected_t1.add((f1.name, f2.name))
                # Type-2: identical AST shape with different tokens
                elif f1.ast_shape == f2.ast_shape:
                    detected_t2.add((f1.name, f2.name))

        return detected_t1, detected_t2, suppressed_trivial

    def verify(self) -> Dict[str, any]:
        assert len(self.ground_truth_t1) == 20
        assert len(self.ground_truth_t2) == 20
        assert len(self.ground_truth_trivial) == 50

        detected_t1, detected_t2, suppressed = self.detect_clones()

        # 1. 100% recall on Type-1 clones
        t1_recall = len(detected_t1 & self.ground_truth_t1) / len(self.ground_truth_t1)
        assert t1_recall == 1.0, f"Expected 1.0 T1 recall, got {t1_recall}"

        # 2. 100% recall on Type-2 clones
        t2_recall = len(detected_t2 & self.ground_truth_t2) / len(self.ground_truth_t2)
        assert t2_recall == 1.0, f"Expected 1.0 T2 recall, got {t2_recall}"

        # 3. 0% trivial noise floor: all 50 trivial accessors suppressed
        assert suppressed == self.ground_truth_trivial
        for f in self.fragments:
            if f.name in self.ground_truth_trivial:
                for pair in detected_t1 | detected_t2:
                    assert f.name not in pair, f"Trivial accessor {f.name} leaked into clone pairs!"

        return {
            "status": "PASS",
            "t1_clones_recall": t1_recall,
            "t2_clones_recall": t2_recall,
            "trivial_accessors_suppressed": len(suppressed),
            "trivial_noise_floor": 0.0,
        }
