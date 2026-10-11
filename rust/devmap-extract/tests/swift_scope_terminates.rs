//! A Swift scope's bindings are computed from inside the scope, and a name the
//! scope does not bind is looked up in the enclosing ones. That lookup used to
//! ask for the bindings of the scope being computed, which asked again, until
//! the stack was gone: `devmap build` aborted on any `for x in xs` whose `xs`
//! was not a parameter or an annotated `let` of the same function.
//!
//! Each case runs on a small-stack thread, so a recursion that does not end
//! kills this test process instead of passing on a roomy main-thread stack.

use devmap_extract::extract_treesitter;

fn completes(source: &'static str) {
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || {
            let _ = extract_treesitter("a.swift", "swift", source);
        })
        .expect("spawn")
        .join()
        .expect("extraction must terminate without overflowing the stack");
}

#[test]
fn a_for_loop_over_an_unbound_collection_terminates() {
    completes("func f() { for h in plugins { g(h) } }\n");
}

#[test]
fn a_tuple_pattern_for_loop_terminates() {
    completes("class M { func f() { for (_, h) in plugins { h.load() } } }\n");
}

#[test]
fn an_optional_binding_of_an_unbound_name_terminates() {
    completes("func f() { if let x = y { g(x) }\n guard let z = w else { return }\n g(z) }\n");
}

#[test]
fn a_let_copied_from_an_unbound_name_terminates() {
    completes("func f() { let a = b\n let c = a\n g(c) }\n");
}

#[test]
fn a_closure_over_an_unbound_collection_terminates() {
    completes("func f() { xs.map { s in for t in ys { g(s, t) } } }\n");
}

#[test]
fn a_nested_function_loop_terminates() {
    completes("func f() { func g() { for h in plugins { k(h) } }\n g() }\n");
}
