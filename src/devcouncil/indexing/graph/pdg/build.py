"""Thin re-export of the PDG build helper (see graph.build).

``load_pdg_layer`` and ``merge_pdg_into_graph`` were re-exported here too. Both
read and wrote the PDG layer inside ``graph.meta["pdg"]``; the layer moved to
its own sidecar in Lane M3 and neither had a production caller afterwards.
"""

from __future__ import annotations

from devcouncil.indexing.graph.build import build_pdg_for_paths

__all__ = ["build_pdg_for_paths"]
