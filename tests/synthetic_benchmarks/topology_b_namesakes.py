"""Topology-B: Dense Namesake Matrix.

10 packages * 50 structs = 500 types.
Every type declares identical methods: execute, render, validate, close.
Asserts:
- Zero false UniqueGlobal bindings across namesake methods.
- Untyped bare calls are relegated to AmbiguousGlobal or Unresolved without fabricating edges.
- Typed receiver calls correctly bind to ReceiverType (Tier 4).
"""

from typing import Dict, List, Set, Tuple


COMMON_METHODS = ["execute", "render", "validate", "close"]


class StructType:
    def __init__(self, package: str, name: str):
        self.package = package
        self.name = name
        self.qualified_name = f"{package}::{name}"
        self.methods = list(COMMON_METHODS)


class TopologyBNamesakes:
    def __init__(self, num_packages: int = 10, structs_per_pkg: int = 50):
        self.num_packages = num_packages
        self.structs_per_pkg = structs_per_pkg
        self.types: Dict[str, StructType] = {}
        self.namesake_index: Dict[str, List[str]] = {m: [] for m in COMMON_METHODS}
        self.build()

    def build(self):
        for p in range(self.num_packages):
            pkg = f"pkg_{p}"
            for s in range(self.structs_per_pkg):
                name = f"Type_{s}"
                st = StructType(package=pkg, name=name)
                self.types[st.qualified_name] = st
                for m in COMMON_METHODS:
                    self.namesake_index[m].append(f"{st.qualified_name}::{m}")

    def resolve_call(self, method_name: str, receiver_type: str = None) -> Tuple[str, str, float]:
        """Resolves a call according to DevCouncil 11-tier resolution ladder rules.
        
        Returns: (tier_name, bound_target, confidence)
        """
        if receiver_type:
            # Statically typed receiver: Tier 4 (ReceiverType, 1.0)
            target = f"{receiver_type}::{method_name}"
            if receiver_type in self.types and method_name in self.types[receiver_type].methods:
                return "ReceiverType", target, 1.0
            return "Unresolved", "", 0.1

        # Untyped call:
        candidates = self.namesake_index.get(method_name, [])
        if len(candidates) == 1:
            return "UniqueGlobal", candidates[0], 0.9
        elif len(candidates) > 1:
            # Over-subscribed namesakes: Tier 7 (AmbiguousGlobal, 0.1)
            return "AmbiguousGlobal", f"{len(candidates)} candidates", 0.1
        else:
            return "Unresolved", "", 0.0

    def verify(self) -> Dict[str, any]:
        total_types = len(self.types)
        assert total_types == self.num_packages * self.structs_per_pkg, f"Expected {self.num_packages * self.structs_per_pkg} types"

        # Verify that each common method has exactly total_types implementations
        for m in COMMON_METHODS:
            assert len(self.namesake_index[m]) == total_types

        # Test untyped calls: MUST NEVER resolve to UniqueGlobal!
        false_unique_globals = 0
        for m in COMMON_METHODS:
            tier, target, conf = self.resolve_call(m, receiver_type=None)
            if tier == "UniqueGlobal":
                false_unique_globals += 1
            assert tier == "AmbiguousGlobal", f"Expected AmbiguousGlobal for {m}, got {tier}"
            assert conf == 0.1

        # Test typed calls: MUST resolve to ReceiverType (1.0)
        sample_type = "pkg_0::Type_0"
        tier, target, conf = self.resolve_call("execute", receiver_type=sample_type)
        assert tier == "ReceiverType"
        assert target == f"{sample_type}::execute"
        assert conf == 1.0

        return {
            "status": "PASS",
            "total_types": total_types,
            "total_methods": total_types * len(COMMON_METHODS),
            "false_unique_globals": false_unique_globals,
            "zero_false_unique_globals": (false_unique_globals == 0),
        }
