"""Topology-D: Dead-Code Island & Cluster.

Exactly 100 dead symbols partitioned across four distinct topology patterns:
- Pattern 1: 40 isolated, unexported leaf functions.
- Pattern 2: 10 mutually recursive function pairs (20 symbols: f <-> g) with 0 inbound callers.
- Pattern 3: 20 unreferenced structures and methods.
- Pattern 4: 20 symbols in an isolated private module cluster.
Total: 100 dead symbols.
Plus active, live application entry points and services.
Asserts:
- Mutual recursion does NOT trick liveness propagation into marking dead clusters as alive.
- Exactly 100 symbols classified as DeadHigh.
- Zero false positives on active live symbols.
"""

from typing import Dict, List, Set, Tuple


class SymbolNode:
    def __init__(self, name: str, is_entrypoint: bool = False, pattern: str = None):
        self.name = name
        self.is_entrypoint = is_entrypoint
        self.pattern = pattern
        self.callers: Set[str] = set()
        self.callees: Set[str] = set()


class TopologyDDeadcode:
    def __init__(self):
        self.symbols: Dict[str, SymbolNode] = {}
        self.ground_truth_dead: Set[str] = set()
        self.ground_truth_live: Set[str] = set()
        self.build()

    def add_edge(self, caller: str, callee: str):
        if caller in self.symbols and callee in self.symbols:
            self.symbols[caller].callees.add(callee)
            self.symbols[callee].callers.add(caller)

    def build(self):
        # 1. Live Application Graph (Entrypoint -> Services -> Handlers)
        for i in range(20):
            sym = f"live_entry_{i}"
            self.symbols[sym] = SymbolNode(name=sym, is_entrypoint=True)
            self.ground_truth_live.add(sym)

        for i in range(50):
            sym = f"live_service_{i}"
            self.symbols[sym] = SymbolNode(name=sym, is_entrypoint=False)
            self.ground_truth_live.add(sym)
            # Connect from entrypoints
            self.add_edge(f"live_entry_{i % 20}", sym)

        # 2. Pattern 1: 40 isolated, unexported leaf functions (0 callers, 0 callees)
        for i in range(40):
            sym = f"dead_leaf_fn_{i}"
            self.symbols[sym] = SymbolNode(name=sym, is_entrypoint=False, pattern="isolated_leaf")
            self.ground_truth_dead.add(sym)

        # 3. Pattern 2: 10 mutually recursive pairs (10 * 2 = 20 symbols)
        for i in range(10):
            sym_a = f"dead_rec_a_{i}"
            sym_b = f"dead_rec_b_{i}"
            self.symbols[sym_a] = SymbolNode(name=sym_a, is_entrypoint=False, pattern="mutual_recursion")
            self.symbols[sym_b] = SymbolNode(name=sym_b, is_entrypoint=False, pattern="mutual_recursion")
            self.ground_truth_dead.add(sym_a)
            self.ground_truth_dead.add(sym_b)
            # Mutual calls
            self.add_edge(sym_a, sym_b)
            self.add_edge(sym_b, sym_a)

        # 4. Pattern 3: 20 unreferenced structures and methods (20 symbols)
        for i in range(20):
            sym = f"DeadStructType_{i}"
            self.symbols[sym] = SymbolNode(name=sym, is_entrypoint=False, pattern="unreferenced_struct")
            self.ground_truth_dead.add(sym)

        # 5. Pattern 4: 20 symbols in an isolated private module cluster (interconnected but no external inbound edge)
        for i in range(20):
            sym = f"dead_cluster_node_{i}"
            self.symbols[sym] = SymbolNode(name=sym, is_entrypoint=False, pattern="isolated_cluster")
            self.ground_truth_dead.add(sym)
        # Add internal ring / mesh edges inside cluster
        for i in range(20):
            curr = f"dead_cluster_node_{i}"
            next_node = f"dead_cluster_node_{(i + 1) % 20}"
            self.add_edge(curr, next_node)

    def analyze_liveness(self) -> Tuple[Set[str], Set[str]]:
        """Breadth-first search forward reachability from entrypoint roots."""
        live_set = set()
        queue = []

        for name, node in self.symbols.items():
            if node.is_entrypoint:
                live_set.add(name)
                queue.append(name)

        while queue:
            curr = queue.pop(0)
            for callee in self.symbols[curr].callees:
                if callee not in live_set:
                    live_set.add(callee)
                    queue.append(callee)

        all_symbols = set(self.symbols.keys())
        dead_set = all_symbols - live_set
        return live_set, dead_set

    def verify(self) -> Dict[str, any]:
        assert len(self.ground_truth_dead) == 100, f"Expected 100 dead symbols, got {len(self.ground_truth_dead)}"

        live_computed, dead_computed = self.analyze_liveness()

        # Check precision and recall
        tp = len(dead_computed & self.ground_truth_dead)
        fp = len(dead_computed - self.ground_truth_dead)
        fn = len(self.ground_truth_dead - dead_computed)

        precision = tp / (tp + fp) if (tp + fp) > 0 else 0.0
        recall = tp / (tp + fn) if (tp + fn) > 0 else 0.0

        assert precision == 1.0, f"False positive dead symbols: {fp}"
        assert recall == 1.0, f"False negative dead symbols: {fn}"
        assert dead_computed == self.ground_truth_dead

        # Specifically check mutual recursion pattern
        for i in range(10):
            assert f"dead_rec_a_{i}" in dead_computed
            assert f"dead_rec_b_{i}" in dead_computed

        # Check isolated cluster
        for i in range(20):
            assert f"dead_cluster_node_{i}" in dead_computed

        return {
            "status": "PASS",
            "total_symbols": len(self.symbols),
            "dead_symbols_count": len(dead_computed),
            "precision": precision,
            "recall": recall,
            "mutual_recursion_handled": True,
            "clusters_handled": True,
        }
