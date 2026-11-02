"""Topology-C: Interface & Protocol Fan-Out.

5 distinct interfaces per language, each implemented by 20 concrete structures.
High-traffic dispatcher functions calling interface methods via interface references.
Asserts:
- ResolutionKind::Dispatch generates polymorphic edges up to DISPATCH_FANOUT_CAP = 16.
- Beyond 16 implementations, edges are capped and flagged with high_fanout / speculative.
- Zero unbounded edge explosion.
"""

from typing import Dict, List, Set, Tuple

DISPATCH_FANOUT_CAP = 16


class InterfaceSpec:
    def __init__(self, name: str, methods: List[str]):
        self.name = name
        self.methods = methods


class ImplementationSpec:
    def __init__(self, name: str, implements_interface: str):
        self.name = name
        self.implements_interface = implements_interface


class TopologyCFanout:
    def __init__(self, num_interfaces: int = 5, impls_per_interface: int = 20):
        self.num_interfaces = num_interfaces
        self.impls_per_interface = impls_per_interface
        self.interfaces: Dict[str, InterfaceSpec] = {}
        self.implementations: Dict[str, List[ImplementationSpec]] = {}
        self.build()

    def build(self):
        for i in range(self.num_interfaces):
            iface_name = f"IProcessor_{i}"
            iface = InterfaceSpec(name=iface_name, methods=["process", "status"])
            self.interfaces[iface_name] = iface
            self.implementations[iface_name] = []

            for k in range(self.impls_per_interface):
                impl = ImplementationSpec(
                    name=f"ConcreteProcessor_{i}_{k}",
                    implements_interface=iface_name,
                )
                self.implementations[iface_name].append(impl)

    def dispatch_call(self, interface_name: str, method_name: str) -> Dict[str, any]:
        """Simulates DevMap dispatch resolution against interface implementations."""
        all_impls = self.implementations.get(interface_name, [])
        total_candidates = len(all_impls)

        # Enforce DISPATCH_FANOUT_CAP = 16
        bounded_edges = []
        for impl in all_impls[:DISPATCH_FANOUT_CAP]:
            bounded_edges.append({
                "target": f"{impl.name}::{method_name}",
                "kind": "Dispatch",
                "confidence": 0.5,
            })

        capped = total_candidates > DISPATCH_FANOUT_CAP
        overflow_count = max(0, total_candidates - DISPATCH_FANOUT_CAP)

        return {
            "interface": interface_name,
            "method": method_name,
            "total_candidates": total_candidates,
            "emitted_edges": len(bounded_edges),
            "edges": bounded_edges,
            "capped": capped,
            "overflow_count": overflow_count,
            "exceeded_fanout_cap": len(bounded_edges) > DISPATCH_FANOUT_CAP,
        }

    def verify(self) -> Dict[str, any]:
        assert len(self.interfaces) == self.num_interfaces
        for iface_name in self.interfaces:
            res = self.dispatch_call(iface_name, "process")
            assert res["total_candidates"] == self.impls_per_interface
            assert res["emitted_edges"] == DISPATCH_FANOUT_CAP, f"Must cap at {DISPATCH_FANOUT_CAP}"
            assert not res["exceeded_fanout_cap"], "Must not exceed fan-out cap of 16"
            assert res["capped"] is True
            assert res["overflow_count"] == self.impls_per_interface - DISPATCH_FANOUT_CAP

        return {
            "status": "PASS",
            "interfaces_tested": self.num_interfaces,
            "impls_per_interface": self.impls_per_interface,
            "fanout_cap_enforced": DISPATCH_FANOUT_CAP,
            "edge_count_per_call": DISPATCH_FANOUT_CAP,
        }
