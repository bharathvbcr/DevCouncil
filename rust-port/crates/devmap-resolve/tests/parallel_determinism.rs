//! Resolution must not depend on how many threads ran it.
//!
//! `Resolver::resolve_all` resolves files in parallel. That is sound only
//! because each file's resolution reads the immutable symbol/type indexes and
//! writes nothing another file observes — but "sound because I read the loop
//! and saw no shared writes" is an argument, not evidence, and the failure mode
//! it guards against is the worst kind: a graph that differs between runs on
//! the same input, with no error and no failing assertion, reproducing only
//! under a particular interleaving on a particular machine.
//!
//! So the property is tested the way it can actually fail. The same corpus is
//! resolved on a one-thread pool and on an eight-thread pool and the two
//! results are compared in full: every edge in order with all of its fields,
//! and every unresolved reference. A merge that dropped a file's output, a
//! `package_groups` merge that lost a key, or any accumulation whose order
//! leaked into the result would show up here as a diff.

use rayon::ThreadPoolBuilder;

use devmap_extract::extract_file;
use devmap_extract::model::*;
use devmap_resolve::*;

/// A corpus with enough cross-file structure that resolution has real work to
/// merge: shared names that resolve ambiguously, inheritance, imports, and
/// methods that collide across files.
fn corpus() -> Vec<Extraction> {
    let mut files = Vec::new();
    for i in 0..40 {
        files.push(extract_file(
            &format!("pkg/mod{i}.py"),
            &format!(
                "import os\n\
                 from pkg.mod{prev} import shared\n\
                 \n\
                 class Base{i}:\n    \
                     def run(self):\n        \
                         return shared()\n\
                 \n\
                 class Child{i}(Base{i}):\n    \
                     def run(self):\n        \
                         return self.helper()\n    \
                     def helper(self):\n        \
                         return {i}\n\
                 \n\
                 def shared():\n    \
                     return {i}\n\
                 \n\
                 def entry{i}():\n    \
                     c = Child{i}()\n    \
                     return c.run() + shared() + os.getpid()\n",
                i = i,
                prev = if i == 0 { 39 } else { i - 1 },
            ),
        ));
    }
    // A TypeScript barrel chain, so `reexport_chains` is a non-empty map to
    // compare rather than two empty ones. Without it the determinism assertion
    // on that field passes over a corpus that produces none, which is a check
    // that cannot fail.
    files.push(extract_file(
        "ts/leaf.ts",
        "export function leafThing() {
  return 1;
}

export const LEAF = 2;
",
    ));
    for i in 0..8 {
        files.push(extract_file(
            &format!("ts/mid{i}.ts"),
            "export * from \"./leaf\";
export { leafThing as aliased } from \"./leaf\";
",
        ));
    }
    files.push(extract_file(
        "ts/barrel.ts",
        &(0..8)
            .map(|i| format!("export * from \"./mid{i}\";\n"))
            .collect::<String>(),
    ));
    files
}

/// Resolve `files` inside a pool of exactly `threads` workers.
///
/// A scoped pool rather than `RAYON_NUM_THREADS`: the global pool is
/// initialised once per process, so an env var could not give one test two
/// different widths, and `install` makes the inner `par_iter` use this pool.
fn resolve_with_threads(files: &[Extraction], threads: usize) -> ResolutionResult {
    let pool = ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .expect("thread pool");
    pool.install(|| {
        let mut resolver = Resolver::new();
        resolver.index_extractions(files);
        resolver.resolve_all(files)
    })
}

/// Every field that reaches an artifact, as a comparable tuple.
///
/// Compared field-by-field rather than by count. Two runs that emit the same
/// *number* of edges while disagreeing about one edge's target or confidence
/// is precisely the silent divergence this test exists to catch, and a length
/// check would pass it.
fn edge_fingerprints(result: &ResolutionResult) -> Vec<String> {
    result
        .edges
        .iter()
        .map(|edge| {
            format!(
                "{}|{}|{}|{}|{:?}|{}|{}",
                edge.source_file,
                edge.target_file,
                edge.source_symbol,
                edge.target_symbol,
                edge.edge_kind,
                edge.confidence.persist_real(),
                edge.details.as_deref().unwrap_or(""),
            )
        })
        .collect()
}

#[test]
fn resolution_is_identical_on_one_thread_and_on_eight() {
    let files = corpus();

    let serial = resolve_with_threads(&files, 1);
    let parallel = resolve_with_threads(&files, 8);

    // A corpus that resolved to nothing would make every assertion below
    // vacuously true.
    assert!(
        serial.edges.len() > 100,
        "corpus resolved to only {} edges — too few to prove anything",
        serial.edges.len()
    );

    let serial_edges = edge_fingerprints(&serial);
    let parallel_edges = edge_fingerprints(&parallel);
    assert_eq!(
        serial_edges.len(),
        parallel_edges.len(),
        "thread count changed the number of resolved edges"
    );
    for (index, (left, right)) in serial_edges.iter().zip(&parallel_edges).enumerate() {
        assert_eq!(
            left, right,
            "edge {index} differs between a 1-thread and an 8-thread resolution"
        );
    }

    let serial_unresolved: Vec<_> = serial
        .unresolved
        .iter()
        .map(|reference| format!("{}|{}", reference.source_file, reference.callee_name))
        .collect();
    let parallel_unresolved: Vec<_> = parallel
        .unresolved
        .iter()
        .map(|reference| format!("{}|{}", reference.source_file, reference.callee_name))
        .collect();
    assert_eq!(
        serial_unresolved, parallel_unresolved,
        "thread count changed which references went unattributed"
    );

    assert_eq!(serial.receiver_types, parallel.receiver_types);
    // Compared for equality again, and this time it means something.
    //
    // It used to assert only that both maps were *empty*, because nothing wrote
    // the field — a check that could not fail, reporting what a check that ran
    // and passed reports. W1.3 computes them, so the emptiness assertion became
    // a claim that was true only because this fixture happens to contain no
    // re-export: it would have failed the moment the fixture grew one, and the
    // comment above it would have been read as evidence the *computation* was
    // wrong.
    assert!(
        !serial.reexport_chains.is_empty(),
        "the corpus must produce re-export chains, or the comparison below is \
         two empty maps and cannot fail"
    );
    assert_eq!(
        serial.reexport_chains, parallel.reexport_chains,
        "thread count changed which re-export chains were computed"
    );
}

/// Repeated runs at the same width must also agree.
///
/// Distinguishes "the parallel merge is order-dependent" from "resolution is
/// nondeterministic for some other reason" — a `HashMap` iteration leaking into
/// output, say. Without this, a failure of the test above would not say which.
#[test]
fn repeated_parallel_resolutions_agree_with_each_other() {
    let files = corpus();
    let first = resolve_with_threads(&files, 8);
    let second = resolve_with_threads(&files, 8);
    assert_eq!(
        edge_fingerprints(&first),
        edge_fingerprints(&second),
        "two 8-thread resolutions of one corpus disagreed"
    );
}
