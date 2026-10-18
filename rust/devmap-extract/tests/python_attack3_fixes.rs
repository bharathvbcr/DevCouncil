//! Controls for the round-three Python fixes: the decorator classifier's
//! builtin reading and the module-scope rebinding facts. Each case fails
//! against the extractor before the fix.

use devmap_extract::extract_file;

fn field_recorded(source: &str) -> bool {
    extract_file("h.py", source)
        .references
        .iter()
        .any(|r| r.assigned_to.as_deref() == Some("self.x"))
}

#[test]
fn builtins_staticmethod_spellings_drop_the_instance() {
    for decorator in [
        "@builtins.staticmethod",
        "@b.staticmethod",
        "@staticmethod",
        "@b.classmethod",
    ] {
        let source = format!(
            "import builtins\nimport builtins as b\nclass H:\n    {decorator}\n    def make(self):\n        self.x = A()\n"
        );
        assert!(
            !field_recorded(&source),
            "{decorator}: a non-instance method wrote a field"
        );
    }
}

#[test]
fn a_foreign_or_rebound_staticmethod_stays_an_instance_method() {
    assert!(field_recorded(
        "import deco\nclass H:\n    @deco.staticmethod\n    def make(self):\n        self.x = A()\n"
    ));
    assert!(field_recorded(
        "import functools as b\nclass H:\n    @b.staticmethod\n    def make(self):\n        self.x = A()\n"
    ));
    assert!(field_recorded(
        "def staticmethod(f):\n    return f\nclass H:\n    @staticmethod\n    def make(self):\n        self.x = A()\n"
    ));
    assert!(field_recorded(
        "from deco import classmethod\nclass H:\n    @classmethod\n    def make(self):\n        self.x = A()\n"
    ));
}

#[test]
fn module_scope_rebinding_forms_are_file_scope_locals() {
    let source = "from a import A, B, C, D, E, F, G\ndel A\nfor B in x:\n    pass\nwith f() as C:\n    pass\nif (D := 1):\n    pass\ntry:\n    pass\nexcept E as exc:\n    pass\ndef reset():\n    global F\n    F = None\ndef inner():\n    for G in x:\n        pass\n";
    let locals = extract_file("h.py", source).scope_locals;
    let file_scope: Vec<&str> = locals
        .iter()
        .filter(|(scope, _)| scope == "h.py")
        .map(|(_, name)| name.as_str())
        .collect();
    for rebound in ["A", "B", "C", "D", "exc", "F"] {
        assert!(
            file_scope.contains(&rebound),
            "{rebound} missing: {file_scope:?}"
        );
    }
    assert!(
        !file_scope.contains(&"G"),
        "a function's own loop target is not module scope"
    );
    assert!(
        !file_scope.contains(&"E"),
        "an exception class named in `except` is a use"
    );
}
