"""Program dependence graph (PDG) layer — CFG, reaching-def, CDG, taint."""

from __future__ import annotations

from devcouncil.indexing.graph.pdg.build import (
    build_pdg_for_paths,
    load_pdg_layer,
    merge_pdg_into_graph,
)
from devcouncil.indexing.graph.pdg.cdg import build_cdg
from devcouncil.indexing.graph.pdg.cfg import CFGResult, block_for_line, build_cfg_for_function
from devcouncil.indexing.graph.pdg.reaching_def import (
    BlockDefUse,
    block_def_use,
    compute_reaching_defs,
)
from devcouncil.indexing.graph.pdg.schema import (
    PDG_VERSION,
    BasicBlock,
    CDGEdge,
    CFGEdge,
    DataDepEdge,
    FilePDG,
    FunctionPDG,
    PDGLayer,
    TaintCategory,
    TaintFinding,
)
from devcouncil.indexing.graph.pdg.taint import analyze_taint
from devcouncil.indexing.graph.pdg.query import (
    explain_pdg_taint,
    explain_taint,
    query_controls,
    query_flows,
    query_pdg_controls,
    query_pdg_flows,
)

__all__ = [
    "PDG_VERSION",
    "BasicBlock",
    "BlockDefUse",
    "CDGEdge",
    "CFGEdge",
    "CFGResult",
    "DataDepEdge",
    "FilePDG",
    "FunctionPDG",
    "PDGLayer",
    "TaintCategory",
    "TaintFinding",
    "analyze_taint",
    "block_def_use",
    "block_for_line",
    "build_cdg",
    "build_cfg_for_function",
    "build_pdg_for_paths",
    "compute_reaching_defs",
    "explain_pdg_taint",
    "explain_taint",
    "load_pdg_layer",
    "merge_pdg_into_graph",
    "query_controls",
    "query_flows",
    "query_pdg_controls",
    "query_pdg_flows",
]
