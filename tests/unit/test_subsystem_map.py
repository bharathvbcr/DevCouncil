"""Unit tests for the shared subsystem/area repo-map helpers."""

from devcouncil.indexing.subsystem_map import (
    are_neighbors,
    can_rule_out_handoffs,
    handoff_paths_established,
    handoff_paths_for_area,
    role_file_total,
    role_files_are_complete,
    role_files_established,
    role_files_for_area,
    area_for_path,
    areas_touched,
    cross_boundary_pairs,
    dependents_of,
    impact_targets,
    is_entry_root,
    neighbors_established,
    neighbors_for_area,
)

_MAP = {
    "files": [{"path": "scripts/tool.py", "area": "scripts"}],
    "subsystems": [
        {"area": "src/ui", "neighbors": ["src/api"]},
        {"area": "src/api", "neighbors": ["src/ui", "src/storage"]},
        {"area": "src/storage", "neighbors": ["src/api"]},
    ],
    "dependents": {"src/api/handler.py": ["src/ui/view.py"]},
}


def test_area_for_path_prefers_longest_prefix():
    assert area_for_path("src/api/handler.py", _MAP) == "src/api"
    assert area_for_path("src/ui/x.py", _MAP) == "src/ui"


def test_area_for_path_falls_back_to_file_entry():
    assert area_for_path("scripts/tool.py", _MAP) == "scripts"


def test_area_for_path_unknown_is_none():
    assert area_for_path("random/unknown.py", _MAP) is None


def test_neighbors_and_adjacency():
    assert neighbors_for_area("src/api", _MAP) == ["src/ui", "src/storage"]
    assert are_neighbors("src/ui", "src/api", _MAP) is True
    assert are_neighbors("src/ui", "src/storage", _MAP) is False
    # Same area is trivially adjacent; unknown side never flags.
    assert are_neighbors("src/ui", "src/ui", _MAP) is True
    assert are_neighbors("src/ui", None, _MAP) is True


def test_dependents_and_impact_targets():
    assert dependents_of("src/api/handler.py", _MAP) == ["src/ui/view.py"]
    deps, neighbors = impact_targets("src/api/handler.py", _MAP)
    assert deps == ["src/ui/view.py"]
    assert neighbors == ["src/ui", "src/storage"]


def test_dependents_total_of_when_truncated():
    from devcouncil.indexing.subsystem_map import dependents_total_of

    data = {
        "dependents": {"src/lib.py": ["a.py", "b.py"]},
        "dependents_total": {"src/lib.py": 99},
    }
    assert dependents_total_of("src/lib.py", data) == 99
    assert dependents_total_of("src/other.py", data) is None
    assert dependents_total_of("src/lib.py", {}) is None


def test_areas_touched_and_cross_boundary_pairs():
    paths = ["src/ui/a.py", "src/storage/b.py", "src/api/c.py"]
    assert areas_touched(paths, _MAP) == ["src/api", "src/storage", "src/ui"]
    # ui<->storage is the only non-neighbor pair among the three.
    assert cross_boundary_pairs(paths, _MAP) == [("src/storage", "src/ui")]


def test_cross_boundary_pairs_empty_when_all_neighbors():
    assert cross_boundary_pairs(["src/ui/a.py", "src/api/b.py"], _MAP) == []


# --- entry-root truncation ------------------------------------------------------
#
# The kernel caps `entry_roots` at 20 for the token budget, so a genuine entry
# root sorting after the cap is absent from the list. Answering `False` there
# reports a capped sample as a complete one: the check could not run, and said
# the same thing as a check that ran and found nothing.


def test_is_entry_root_true_for_a_listed_path():
    data = {"entry_roots": ["src/main.py", "src/cli.py"]}
    assert is_entry_root("src/main.py", data) is True


def test_is_entry_root_false_when_the_list_is_provably_complete():
    data = {
        "entry_roots": ["src/main.py"],
        "liveness_meta": {"entry_roots": {"shown": 1, "total": 1, "truncated": False}},
    }
    assert is_entry_root("src/other.py", data) is False


def test_is_entry_root_unknown_when_the_list_is_truncated():
    data = {
        "entry_roots": ["src/main.py"],
        "liveness_meta": {"entry_roots": {"shown": 1, "total": 25, "truncated": True}},
    }
    assert is_entry_root("src/other.py", data) is None


def test_is_entry_root_listed_path_stays_true_under_truncation():
    """Presence is still provable when the list is capped; only absence is not."""
    data = {
        "entry_roots": ["src/main.py"],
        "liveness_meta": {"entry_roots": {"shown": 1, "total": 25, "truncated": True}},
    }
    assert is_entry_root("src/main.py", data) is True


def test_is_entry_root_unknown_when_shown_is_below_total():
    """`truncated` absent but the counts disagree: still not a knowable `False`."""
    data = {
        "entry_roots": ["src/main.py"],
        "liveness_meta": {"entry_roots": {"shown": 1, "total": 25}},
    }
    assert is_entry_root("src/other.py", data) is None


def test_is_entry_root_without_metadata_keeps_the_prior_answer():
    """A map carrying no truncation metadata claims none; behaviour is unchanged."""
    assert is_entry_root("src/other.py", {"entry_roots": ["src/main.py"]}) is False
    assert is_entry_root("src/other.py", None) is False


def test_is_entry_root_survives_malformed_metadata():
    for meta in ("nonsense", {"entry_roots": "nonsense"}, {"entry_roots": {"total": "x"}}):
        assert is_entry_root("src/other.py", {"entry_roots": ["src/main.py"], "liveness_meta": meta}) is False


# --- neighbours: absence of evidence is not evidence of absence -------------------

_NO_NEIGHBORS_COMPUTED = {
    "subsystems": [
        {"area": "src/ui", "neighbors": []},
        {"area": "src/api", "neighbors": []},
        {"area": "src/storage", "neighbors": []},
    ],
}

_NEIGHBORS_COMPUTED_AND_EMPTY = {
    "meta": {"devmap_rust": {"neighbors_computed": True}},
    "subsystems": [
        {"area": "src/ui", "neighbors": []},
        {"area": "src/api", "neighbors": []},
        {"area": "src/storage", "neighbors": []},
    ],
}


def test_adjacency_is_unknown_when_the_map_never_computed_neighbors():
    """An all-empty ``neighbors`` field is the producer's silence, not an answer.

    The kernel writes ``"neighbors": []`` as a literal for every subsystem
    (``devmap-query/src/manifest.rs``), so reading that as "not adjacent" makes
    every repository look like one with no adjacent subsystems at all.
    """
    assert neighbors_established(_NO_NEIGHBORS_COMPUTED) is False
    assert are_neighbors("src/ui", "src/storage", _NO_NEIGHBORS_COMPUTED) is None
    # A same-area or unknown-side answer needs no evidence and stays definite.
    assert are_neighbors("src/ui", "src/ui", _NO_NEIGHBORS_COMPUTED) is True
    assert are_neighbors("src/ui", None, _NO_NEIGHBORS_COMPUTED) is True
    # And an unknown relation is not a crossing: nothing can be claimed.
    assert cross_boundary_pairs(
        ["src/ui/a.py", "src/storage/b.py"], _NO_NEIGHBORS_COMPUTED
    ) == []


def test_the_provenance_marker_turns_an_empty_list_into_a_real_answer():
    """With the producer saying it computed them, empty means empty."""
    assert neighbors_established(_NEIGHBORS_COMPUTED_AND_EMPTY) is True
    assert are_neighbors("src/ui", "src/storage", _NEIGHBORS_COMPUTED_AND_EMPTY) is False
    assert cross_boundary_pairs(
        ["src/ui/a.py", "src/storage/b.py"], _NEIGHBORS_COMPUTED_AND_EMPTY
    ) == [("src/storage", "src/ui")]


def test_a_populated_neighbor_list_is_its_own_evidence():
    """No marker needed: a map that names a neighbour computed neighbours."""
    assert neighbors_established(_MAP) is True
    assert are_neighbors("src/ui", "src/storage", _MAP) is False


_NEIGHBORS_TRUNCATED = {
    "meta": {"devmap_rust": {"neighbors_computed": True}},
    "liveness_meta": {
        "subsystems": {
            "neighbors_shown": 4,
            "neighbors_total": 9,
            "neighbors_truncated": True,
            "neighbors_endpoints_unresolved": 0,
        }
    },
    "subsystems": [
        {"area": "src/ui", "neighbors": ["src/api"]},
        {"area": "src/api", "neighbors": ["src/ui"]},
        {"area": "src/storage", "neighbors": []},
    ],
}

_NEIGHBORS_WITH_UNPLACED_ENDPOINTS = {
    "meta": {"devmap_rust": {"neighbors_computed": True}},
    "liveness_meta": {
        "subsystems": {
            "neighbors_shown": 2,
            "neighbors_total": 2,
            "neighbors_truncated": False,
            "neighbors_endpoints_unresolved": 3,
        }
    },
    "subsystems": [
        {"area": "src/ui", "neighbors": ["src/api"]},
        {"area": "src/api", "neighbors": ["src/ui"]},
        {"area": "src/storage", "neighbors": []},
    ],
}


def test_a_capped_neighbor_list_cannot_answer_a_negative():
    """The writer bounds each area's list; absence from a capped list proves nothing.

    Same rule as ``is_entry_root`` two functions down. A *listed* neighbour is
    still a neighbour — presence survives any cap — so only the negative
    degrades.
    """
    assert are_neighbors("src/ui", "src/api", _NEIGHBORS_TRUNCATED) is True
    assert are_neighbors("src/ui", "src/storage", _NEIGHBORS_TRUNCATED) is None
    assert cross_boundary_pairs(
        ["src/ui/a.py", "src/storage/b.py"], _NEIGHBORS_TRUNCATED
    ) == []


def test_couplings_the_map_could_not_place_also_withhold_the_negative():
    """`neighbors_endpoints_unresolved` counts adjacency that was not computed.

    "No neighbours" and "some couplings could not be placed in an area" are
    different answers, and the second cannot rule anything out.
    """
    assert are_neighbors("src/ui", "src/api", _NEIGHBORS_WITH_UNPLACED_ENDPOINTS) is True
    assert are_neighbors("src/ui", "src/storage", _NEIGHBORS_WITH_UNPLACED_ENDPOINTS) is None


def test_a_complete_computed_relation_still_answers_no():
    """The metadata present and claiming completeness keeps the negative definite."""
    complete = dict(_NEIGHBORS_TRUNCATED)
    complete["liveness_meta"] = {
        "subsystems": {
            "neighbors_shown": 2,
            "neighbors_total": 2,
            "neighbors_truncated": False,
            "neighbors_endpoints_unresolved": 0,
        }
    }
    assert are_neighbors("src/ui", "src/storage", complete) is False


# --- handoff_paths: the same Class A defect the neighbor relation carried ------
#
# `build_repo_map_value` wrote `"handoff_paths": []` for every subsystem for the
# field's whole life, so an empty list is what a producer that stubs the field
# emits *and* what a subsystem reaching nothing else would emit. The guides sent
# agents to the field regardless, so the stub answered "there is no flow here".

_NO_HANDOFFS_COMPUTED = {
    "subsystems": [
        {"area": "src/ui", "handoff_paths": []},
        {"area": "src/api", "handoff_paths": []},
    ],
}

_HANDOFFS_COMPUTED_AND_EMPTY = {
    "meta": {"devmap_rust": {"handoff_paths_computed": True}},
    "subsystems": [
        {"area": "src/ui", "handoff_paths": []},
        {"area": "src/api", "handoff_paths": []},
    ],
}

_HANDOFFS_POPULATED = {
    "subsystems": [
        {"area": "src/ui", "handoff_paths": ["src/ui/page.py -> src/api/route.py"]},
        {"area": "src/api", "handoff_paths": []},
    ],
}


def test_handoffs_are_unknown_when_the_map_never_computed_them():
    """The defect itself: a stubbed field must not answer "nothing crosses"."""
    assert handoff_paths_established(_NO_HANDOFFS_COMPUTED) is False
    assert can_rule_out_handoffs(_NO_HANDOFFS_COMPUTED) is False
    assert handoff_paths_for_area("src/ui", _NO_HANDOFFS_COMPUTED) is None


def test_the_provenance_marker_turns_an_empty_handoff_list_into_a_real_answer():
    """`meta.devmap_rust.handoff_paths_computed` is what makes the empty list evidence."""
    assert handoff_paths_established(_HANDOFFS_COMPUTED_AND_EMPTY) is True
    assert can_rule_out_handoffs(_HANDOFFS_COMPUTED_AND_EMPTY) is True
    assert handoff_paths_for_area("src/ui", _HANDOFFS_COMPUTED_AND_EMPTY) == []


def test_a_populated_handoff_list_is_its_own_evidence():
    """A map written before the marker existed still demonstrates it computed them."""
    assert handoff_paths_established(_HANDOFFS_POPULATED) is True
    assert handoff_paths_for_area("src/ui", _HANDOFFS_POPULATED) == [
        "src/ui/page.py -> src/api/route.py"
    ]
    # And the sibling's empty list is now an answer, because the producer showed
    # it computes the field.
    assert handoff_paths_for_area("src/api", _HANDOFFS_POPULATED) == []


def test_a_capped_handoff_list_cannot_answer_a_negative():
    """A truncated relation is a sample; a negative read off it is a sample answering as a whole."""
    truncated = {
        "meta": {"devmap_rust": {"handoff_paths_computed": True}},
        "liveness_meta": {"subsystems": {"handoff_paths_truncated": True}},
        "subsystems": [{"area": "src/ui", "handoff_paths": []}],
    }
    assert handoff_paths_established(truncated) is True
    assert can_rule_out_handoffs(truncated) is False
    assert handoff_paths_for_area("src/ui", truncated) is None


def test_a_shown_below_total_handoff_count_also_withholds_the_negative():
    """The counts say the same thing the boolean does, and are read the same way."""
    capped = {
        "meta": {"devmap_rust": {"handoff_paths_computed": True}},
        "liveness_meta": {
            "subsystems": {"handoff_paths_shown": 16, "handoff_paths_total": 48}
        },
        "subsystems": [{"area": "src/ui", "handoff_paths": []}],
    }
    assert can_rule_out_handoffs(capped) is False
    assert handoff_paths_for_area("src/ui", capped) is None


def test_the_handoff_cap_is_asked_about_separately_from_the_neighbor_cap():
    """Whole neighbor lists say nothing about whether the handoff lists are whole.

    An area with two neighbors can reach them through fifty file pairs, so
    reusing `neighbors_truncated` here would let a capped handoff relation
    answer a negative off a complete neighbor one.
    """
    whole_neighbors_capped_handoffs = {
        "meta": {
            "devmap_rust": {"neighbors_computed": True, "handoff_paths_computed": True}
        },
        "liveness_meta": {
            "subsystems": {
                "neighbors_truncated": False,
                "neighbors_shown": 4,
                "neighbors_total": 4,
                "handoff_paths_truncated": True,
            }
        },
        "subsystems": [{"area": "src/ui", "neighbors": ["src/api"], "handoff_paths": []}],
    }
    assert can_rule_out_handoffs(whole_neighbors_capped_handoffs) is False


def test_a_listed_handoff_stays_listed_under_truncation():
    """Only the negative degrades: a named crossing is a crossing whatever else is cut."""
    truncated = {
        "meta": {"devmap_rust": {"handoff_paths_computed": True}},
        "liveness_meta": {"subsystems": {"handoff_paths_truncated": True}},
        "subsystems": [
            {"area": "src/ui", "handoff_paths": ["src/ui/page.py -> src/api/route.py"]}
        ],
    }
    assert handoff_paths_for_area("src/ui", truncated) == [
        "src/ui/page.py -> src/api/route.py"
    ]


def test_unplaceable_endpoints_also_withhold_the_handoff_negative():
    """One sweep feeds both relations, so its rejects bear on both."""
    unplaced = {
        "meta": {"devmap_rust": {"handoff_paths_computed": True}},
        "liveness_meta": {"subsystems": {"neighbors_endpoints_unresolved": 3}},
        "subsystems": [{"area": "src/ui", "handoff_paths": []}],
    }
    assert can_rule_out_handoffs(unplaced) is False


def test_handoff_readers_survive_malformed_metadata_and_unknown_areas():
    """Never raises on odd input — a malformed map degrades to unknown."""
    assert handoff_paths_established(None) is False
    assert handoff_paths_established({"meta": "nope", "subsystems": "nope"}) is False
    assert handoff_paths_for_area(None, _HANDOFFS_POPULATED) is None
    assert handoff_paths_for_area("src/nowhere", _HANDOFFS_POPULATED) is None
    assert can_rule_out_handoffs({"liveness_meta": {"subsystems": "nope"}}) is False


# --- role_files: the same Class A defect, third instance --------------------
#
# `build_repo_map_value` wrote `"role_files": {}` for every subsystem while the
# generated agent guide's step 5 told agents to navigate by it, and
# `verification/test_resolver.py` resolved subsystem tests through
# `role_files["tests"]` — so the stub silently removed a fallback.

_NO_ROLES_COMPUTED = {"subsystems": [{"area": "src/ui", "role_files": {}}]}

_ROLES_COMPUTED_AND_EMPTY = {
    "meta": {"devmap_rust": {"role_files_computed": True}},
    "subsystems": [{"area": "src/ui", "role_files": {}}],
}

_ROLES_POPULATED = {
    "subsystems": [
        {
            "area": "src/ui",
            "role_files": {"tests": ["src/ui/test_a.py", "src/ui/test_b.py"]},
            "role_file_counts": {"tests": 37},
        },
        {"area": "src/api", "role_files": {}},
    ],
}


def test_roles_are_unknown_when_the_map_never_computed_them():
    """A stubbed mapping must not answer "this subsystem has no roles"."""
    assert role_files_established(_NO_ROLES_COMPUTED) is False
    assert role_files_for_area("src/ui", _NO_ROLES_COMPUTED) is None


def test_the_provenance_marker_turns_an_empty_role_mapping_into_an_answer():
    assert role_files_established(_ROLES_COMPUTED_AND_EMPTY) is True
    assert role_files_for_area("src/ui", _ROLES_COMPUTED_AND_EMPTY) == {}


def test_a_populated_role_mapping_is_its_own_evidence():
    assert role_files_established(_ROLES_POPULATED) is True
    assert role_files_for_area("src/ui", _ROLES_POPULATED) == {
        "tests": ["src/ui/test_a.py", "src/ui/test_b.py"]
    }
    assert role_files_for_area("src/api", _ROLES_POPULATED) == {}


def test_the_capped_bucket_never_reports_itself_as_the_inventory():
    """Two names in `tests` with a count of 37 is a sample, and must read as one."""
    listed = role_files_for_area("src/ui", _ROLES_POPULATED)
    assert listed is not None and len(listed["tests"]) == 2
    assert role_file_total("src/ui", "tests", _ROLES_POPULATED) == 37


def test_a_missing_count_is_unknown_rather_than_the_sample_size():
    """Absent `role_file_counts` must not be answered with `len(bucket)`."""
    no_counts = {"subsystems": [{"area": "src/ui", "role_files": {"tests": ["a.py"]}}]}
    assert role_file_total("src/ui", "tests", no_counts) is None
    assert role_file_total("src/ui", "missing_role", _ROLES_POPULATED) is None


def test_capped_role_buckets_are_not_complete():
    """The buckets are documented as a capped sample, so completeness is False."""
    capped = {
        "meta": {"devmap_rust": {"role_files_computed": True}},
        "liveness_meta": {"subsystems": {"role_files_truncated": True}},
        "subsystems": [{"area": "src/ui", "role_files": {"tests": ["a.py"]}}],
    }
    assert role_files_established(capped) is True
    assert role_files_are_complete(capped) is False


def test_role_completeness_reads_its_own_counters_not_the_neighbor_ones():
    """Whole neighbor lists say nothing about whether the role buckets are whole."""
    mixed = {
        "meta": {"devmap_rust": {"neighbors_computed": True, "role_files_computed": True}},
        "liveness_meta": {
            "subsystems": {
                "neighbors_truncated": False,
                "role_files_shown": 78,
                "role_files_total": 628,
            }
        },
        "subsystems": [{"area": "src/ui", "role_files": {"tests": ["a.py"]}}],
    }
    assert role_files_are_complete(mixed) is False


def test_role_readers_survive_malformed_metadata_and_unknown_areas():
    assert role_files_established(None) is False
    assert role_files_established({"meta": "nope", "subsystems": "nope"}) is False
    assert role_files_for_area(None, _ROLES_POPULATED) is None
    assert role_files_for_area("src/nowhere", _ROLES_POPULATED) is None
    assert role_file_total(None, "tests", _ROLES_POPULATED) is None
    assert role_files_are_complete({"liveness_meta": {"subsystems": "nope"}}) is False
