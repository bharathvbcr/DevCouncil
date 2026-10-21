use devmap_analyze::*;
use devmap_extract::extract_file;
use devmap_extract::model::*;
use devmap_resolve::*;

fn reports(sources: &[(&str, &str)]) -> Vec<DeadSymbolReport> {
    let extractions: Vec<Extraction> = sources.iter().map(|(p, s)| extract_file(p, s)).collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    analyze_liveness(&extractions, &resolution)
}

/// live symbols that are reported dead
fn assert_live(path: &str, src: &str, live: &[&str]) {
    let r = reports(&[(path, src)]);
    let bad: Vec<String> = r
        .iter()
        .filter(|x| !x.is_exempt && live.contains(&x.symbol_name.as_str()))
        .map(|x| format!("{} conf={:?}", x.symbol_name, x.confidence))
        .collect();
    assert!(bad.is_empty(), "LIVE reported dead in {path}: {bad:#?}");
}

macro_rules! py {
    ($name:ident, $live:expr, $src:expr) => {
        #[test]
        fn $name() {
            assert_live("tools/s.py", $src, $live);
        }
    };
}
macro_rules! js {
    ($name:ident, $path:expr, $live:expr, $src:expr) => {
        #[test]
        fn $name() {
            assert_live($path, $src, $live);
        }
    };
}

py!(py_dispatch_dict, &["cmd_a","cmd_b"], "#!/usr/bin/env python3\nimport sys\ndef cmd_a():\n    return 1\ndef cmd_b():\n    return 2\nCOMMANDS = {\"a\": cmd_a, \"b\": cmd_b}\nif __name__ == \"__main__\":\n    COMMANDS[sys.argv[1]]()\n");
py!(py_dispatch_list, &["cmd_a"], "#!/usr/bin/env python3\ndef cmd_a():\n    return 1\nSTEPS = [cmd_a]\nif __name__ == \"__main__\":\n    for s in STEPS:\n        s()\n");
py!(py_set_defaults, &["handler"], "#!/usr/bin/env python3\nimport argparse\ndef handler(a):\n    return 1\nif __name__ == \"__main__\":\n    p = argparse.ArgumentParser()\n    p.set_defaults(func=handler)\n    a = p.parse_args()\n    a.func(a)\n");
py!(
    py_atexit,
    &["bye"],
    "#!/usr/bin/env python3\nimport atexit\ndef bye():\n    pass\natexit.register(bye)\n"
);
py!(py_signal, &["on_int"], "#!/usr/bin/env python3\nimport signal\ndef on_int(s, f):\n    pass\nsignal.signal(signal.SIGINT, on_int)\n");
py!(py_thread, &["work"], "#!/usr/bin/env python3\nimport threading\ndef work():\n    pass\nif __name__ == \"__main__\":\n    t = threading.Thread(target=work)\n    t.start()\n");
py!(py_map, &["sq"], "#!/usr/bin/env python3\ndef sq(x):\n    return x*x\nif __name__ == \"__main__\":\n    print(list(map(sq, [1,2])))\n");
py!(py_alias, &["main"], "#!/usr/bin/env python3\ndef main():\n    return 1\nrun = main\nif __name__ == \"__main__\":\n    run()\n");
py!(py_globals, &["do_x"], "#!/usr/bin/env python3\nimport sys\ndef do_x():\n    return 1\nif __name__ == \"__main__\":\n    globals()[\"do_\" + sys.argv[1]]()\n");
py!(py_decorator_route, &["index"], "#!/usr/bin/env python3\nfrom flask import Flask\napp = Flask(__name__)\n@app.route('/')\ndef index():\n    return 'x'\nif __name__ == \"__main__\":\n    app.run()\n");
py!(py_custom_registry, &["handler"], "#!/usr/bin/env python3\nREG = {}\ndef register(f):\n    REG[f.__name__] = f\n    return f\n@register\ndef handler():\n    return 1\nif __name__ == \"__main__\":\n    REG['handler']()\n");
py!(py_class_guard, &["Tool","run"], "#!/usr/bin/env python3\nclass Tool:\n    def run(self):\n        return 1\nif __name__ == \"__main__\":\n    Tool().run()\n");
py!(py_all, &["exported"], "#!/usr/bin/env python3\n__all__ = ['exported']\ndef exported():\n    return 1\nif __name__ == \"__main__\":\n    pass\n");
py!(py_fstring_const, &["BANNER"], "#!/usr/bin/env python3\nBANNER = 'hi'\nif __name__ == \"__main__\":\n    print(f\"{BANNER}\")\n");
py!(
    py_percent_const,
    &["FMT"],
    "#!/usr/bin/env python3\nFMT = '%s'\nif __name__ == \"__main__\":\n    print(FMT % 3)\n"
);
py!(py_const_in_live_fn, &["LIMIT","go"], "#!/usr/bin/env python3\nLIMIT = 3\ndef go():\n    return LIMIT\nif __name__ == \"__main__\":\n    go()\n");
py!(
    py_reversed_guard,
    &["main"],
    "#!/usr/bin/env python3\ndef main():\n    return 1\nif \"__main__\" == __name__:\n    main()\n"
);
py!(
    py_single_quote_guard,
    &["main"],
    "#!/usr/bin/env python3\ndef main():\n    return 1\nif __name__ == '__main__':\n    main()\n"
);
py!(py_cells, &["load","plot"], "#!/usr/bin/env python3\n# %%\ndef load():\n    return 1\n# %%\ndef plot(d):\n    return d\n# %%\nplot(load())\n");
py!(
    py_shebang_only_no_call,
    &["main"],
    "#!/usr/bin/env python3\ndef main():\n    return 1\n"
);
py!(
    py_guard_raise_systemexit,
    &["main"],
    "def main():\n    return 1\nif __name__ == '__main__':\n    raise SystemExit(main())\n"
);
py!(py_asyncio_run, &["amain"], "#!/usr/bin/env python3\nimport asyncio\nasync def amain():\n    return 1\nif __name__ == '__main__':\n    asyncio.run(amain())\n");
py!(py_main_in_try, &["main"], "#!/usr/bin/env python3\ndef main():\n    return 1\nif __name__ == '__main__':\n    try:\n        main()\n    except KeyboardInterrupt:\n        pass\n");
py!(py_getattr_module, &["cmd_x"], "#!/usr/bin/env python3\nimport sys\ndef cmd_x():\n    return 1\nif __name__ == '__main__':\n    getattr(sys.modules[__name__], 'cmd_' + 'x')()\n");
py!(py_lambda_table, &["a"], "#!/usr/bin/env python3\ndef a():\n    return 1\nT = {'a': lambda: a()}\nif __name__ == '__main__':\n    T['a']()\n");
py!(py_functools_partial, &["a"], "#!/usr/bin/env python3\nimport functools\ndef a(x):\n    return 1\nif __name__ == '__main__':\n    functools.partial(a, 1)()\n");
py!(py_executor_submit, &["a"], "#!/usr/bin/env python3\nfrom concurrent.futures import ThreadPoolExecutor\ndef a():\n    return 1\nif __name__ == '__main__':\n    ThreadPoolExecutor().submit(a)\n");
py!(py_sorted_key, &["k"], "#!/usr/bin/env python3\ndef k(x):\n    return x\nif __name__ == '__main__':\n    sorted([1], key=k)\n");
py!(py_enum_class_guard, &["Color"], "#!/usr/bin/env python3\nimport enum\nclass Color(enum.Enum):\n    RED=1\nif __name__ == '__main__':\n    print(Color.RED)\n");
py!(py_dataclass_guard, &["Cfg"], "#!/usr/bin/env python3\nfrom dataclasses import dataclass\n@dataclass\nclass Cfg:\n    x: int = 1\nif __name__ == '__main__':\n    print(Cfg())\n");
py!(
    py_main_before_def,
    &["main"],
    "#!/usr/bin/env python3\nif __name__ == '__main__':\n    main()\ndef main():\n    return 1\n"
);
py!(py_nested_fn_in_guard, &["inner"], "#!/usr/bin/env python3\nif __name__ == '__main__':\n    def inner():\n        return 1\n    inner()\n");
py!(py_type_annotation_use, &["Opts"], "#!/usr/bin/env python3\nfrom typing import NamedTuple\nclass Opts(NamedTuple):\n    a: int\ndef main(o: Opts) -> int:\n    return o.a\nif __name__ == '__main__':\n    main(None)\n");
py!(py_cli_subparser, &["cmd_run"], "#!/usr/bin/env python3\nimport argparse\ndef cmd_run(a):\n    pass\ndef build():\n    p = argparse.ArgumentParser()\n    s = p.add_subparsers()\n    r = s.add_parser('run')\n    r.set_defaults(func=cmd_run)\n    return p\nif __name__ == '__main__':\n    a = build().parse_args()\n    a.func(a)\n");
py!(py_unittest_main, &["TestX"], "#!/usr/bin/env python3\nimport unittest\nclass TestX(unittest.TestCase):\n    def test_a(self):\n        pass\nif __name__ == '__main__':\n    unittest.main()\n");
py!(py_pytest_fn, &["test_a"], "#!/usr/bin/env python3\ndef test_a():\n    assert True\nif __name__ == '__main__':\n    import pytest\n    pytest.main([__file__])\n");
py!(py_property_in_class, &["Tool"], "#!/usr/bin/env python3\nclass Tool:\n    @property\n    def v(self):\n        return 1\nif __name__ == '__main__':\n    print(Tool().v)\n");
py!(py_cmd_class_dispatch, &["Cmd"], "#!/usr/bin/env python3\nimport cmd\nclass Cmd(cmd.Cmd):\n    def do_x(self, a):\n        pass\nif __name__ == '__main__':\n    Cmd().cmdloop()\n");
py!(py_with_context, &["ctx"], "#!/usr/bin/env python3\nimport contextlib\n@contextlib.contextmanager\ndef ctx():\n    yield\nif __name__ == '__main__':\n    with ctx():\n        pass\n");

js!(js_require_main, "bin/t.js", &["main"], "#!/usr/bin/env node\nfunction main() { return 1; }\nif (require.main === module) { main(); }\n");
js!(js_exports_obj, "bin/t.js", &["a","b"], "#!/usr/bin/env node\nfunction a() { return 1; }\nfunction b() { return 2; }\nmodule.exports = { a, b };\n");
js!(
    js_exports_assign,
    "bin/t.js",
    &["a"],
    "#!/usr/bin/env node\nfunction a() { return 1; }\nexports.a = a;\n"
);
js!(
    js_process_on,
    "bin/t.js",
    &["bye"],
    "#!/usr/bin/env node\nfunction bye() {}\nprocess.on('exit', bye);\n"
);
js!(js_commander, "bin/t.js", &["act"], "#!/usr/bin/env node\nconst { program } = require('commander');\nfunction act() {}\nprogram.command('x').action(act);\nprogram.parse();\n");
js!(
    js_export_default,
    "bin/t.js",
    &["main"],
    "#!/usr/bin/env node\nexport default function main() { return 1; }\n"
);
js!(
    js_export_default_ident,
    "bin/t.js",
    &["main"],
    "#!/usr/bin/env node\nfunction main() { return 1; }\nexport default main;\n"
);
js!(
    js_named_export,
    "bin/t.js",
    &["a"],
    "#!/usr/bin/env node\nexport function a() { return 1; }\n"
);
js!(
    js_callback,
    "bin/t.js",
    &["cb"],
    "#!/usr/bin/env node\nfunction cb() {}\nsetTimeout(cb, 10);\n"
);
js!(
    js_array_map,
    "bin/t.js",
    &["f"],
    "#!/usr/bin/env node\nfunction f(x) { return x; }\n[1].map(f);\n"
);
js!(
    js_table,
    "bin/t.js",
    &["a"],
    "#!/usr/bin/env node\nfunction a() {}\nconst T = { a };\nT[process.argv[2]]();\n"
);
js!(
    js_class,
    "bin/t.js",
    &["Tool"],
    "#!/usr/bin/env node\nclass Tool { run() {} }\nnew Tool().run();\n"
);
js!(
    js_const_arrow,
    "bin/t.js",
    &["main"],
    "#!/usr/bin/env node\nconst main = () => 1;\nmain();\n"
);
js!(
    js_async_main,
    "bin/t.js",
    &["main"],
    "#!/usr/bin/env node\nasync function main() { return 1; }\nmain().catch(console.error);\n"
);
js!(js_const_used_in_main, "bin/t.js", &["URL_BASE","main"], "#!/usr/bin/env node\nconst URL_BASE = 'x';\nfunction main() { return `${URL_BASE}/a`; }\nmain();\n");
js!(
    js_ts_export_const,
    "bin/t.ts",
    &["handler"],
    "#!/usr/bin/env node\nexport const handler = async () => 1;\n"
);
js!(js_ts_interface, "bin/t.ts", &["Opts"], "#!/usr/bin/env node\ninterface Opts { a: number }\nfunction main(o: Opts) { return o.a; }\nmain({a:1});\n");
js!(js_ts_type_only, "bin/t.ts", &["Opts"], "#!/usr/bin/env node\ntype Opts = { a: number };\nfunction main(o: Opts) { return o.a; }\nmain({a:1});\n");
js!(js_commander_chain, "bin/t.js", &["a","b"], "#!/usr/bin/env node\nfunction a() {}\nfunction b() {}\nrequire('commander').program.command('a').action(a).command('b').action(b);\n");
js!(
    js_module_exports_fn,
    "bin/t.js",
    &["main"],
    "#!/usr/bin/env node\nmodule.exports = function main() { return 1; };\n"
);
js!(
    js_alias_call,
    "bin/t.js",
    &["main"],
    "#!/usr/bin/env node\nfunction main() {}\nconst run = main;\nrun();\n"
);
js!(js_ts_import_meta, "bin/t.ts", &["main"], "#!/usr/bin/env node\nfunction main() {}\nif (import.meta.url === `file://${process.argv[1]}`) { main(); }\n");
js!(
    js_ts_exported_main,
    "bin/t.ts",
    &["main"],
    "#!/usr/bin/env node\nexport async function main() {}\n"
);
js!(
    js_mjs,
    "bin/t.mjs",
    &["a"],
    "#!/usr/bin/env node\nexport function a() {}\n"
);
js!(
    js_tsx_comp,
    "bin/t.tsx",
    &["App"],
    "#!/usr/bin/env node\nfunction App() { return null; }\nexport default App;\n"
);
js!(
    js_cjs_ext,
    "bin/t.cjs",
    &["a"],
    "#!/usr/bin/env node\nfunction a() {}\nmodule.exports = a;\n"
);

// both a script and a library: another module imports from the script.
#[test]
fn py_script_imported_by_module() {
    let r = reports(&[
        ("tools/s.py", "#!/usr/bin/env python3\ndef util():\n    return 1\nif __name__ == '__main__':\n    util()\n"),
        ("tools/other.py", "from tools.s import util\ndef f():\n    return util()\n"),
        ("tools/third.py", "import s\ndef g():\n    return s.util()\n"),
    ]);
    let bad: Vec<_> = r
        .iter()
        .filter(|x| !x.is_exempt && x.file_path == "tools/s.py")
        .map(|x| x.symbol_name.clone())
        .collect();
    assert!(bad.is_empty(), "{bad:?}");
}

// Languages that must stay exempt.
fn exempt_all(path: &str, src: &str) {
    let r = reports(&[(path, src)]);
    let bad: Vec<_> = r
        .iter()
        .filter(|x| !x.is_exempt && x.file_path == path)
        .map(|x| format!("{} {:?}", x.symbol_name, x.confidence))
        .collect();
    assert!(bad.is_empty(), "{path} lost its file exemption: {bad:?}");
}
#[test]
fn ruby_exempt() {
    exempt_all(
        "bin/t.rb",
        "#!/usr/bin/env ruby\ndef a\n  1\nend\nclass K\n  def b; 2; end\nend\n",
    );
}
#[test]
fn php_exempt() {
    exempt_all(
        "bin/t.php",
        "#!/usr/bin/env php\n<?php\nfunction a() { return 1; }\nclass K { function b() {} }\n",
    );
}
#[test]
fn shell_exempt() {
    exempt_all(
        "bin/t.sh",
        "#!/bin/sh\nhelper() { echo 1; }\nother() { echo 2; }\nother\n",
    );
}
#[test]
fn bash_exempt() {
    exempt_all("bin/t.bash", "#!/bin/bash\nhelper() { echo 1; }\n");
}
#[test]
fn lua_exempt() {
    exempt_all(
        "bin/t.lua",
        "#!/usr/bin/env lua\nlocal function a() return 1 end\nfunction b() return 2 end\n",
    );
}
#[test]
fn r_exempt() {
    exempt_all(
        "bin/t.R",
        "#!/usr/bin/env Rscript\na <- function() 1\nb <- function() 2\n",
    );
}
#[test]
fn swift_exempt() {
    exempt_all(
        "bin/t.swift",
        "#!/usr/bin/env swift\nfunc a() -> Int { 1 }\nfunc b() -> Int { 2 }\nprint(a())\n",
    );
}
#[test]
fn kotlin_exempt() {
    exempt_all(
        "bin/t.kts",
        "#!/usr/bin/env kotlin\nfun a() = 1\nfun b() = 2\nprintln(a())\n",
    );
}
#[test]
fn scala_exempt() {
    exempt_all(
        "bin/t.scala",
        "#!/usr/bin/env scala\ndef a = 1\ndef b = 2\nprintln(a)\n",
    );
}
#[test]
fn dart_exempt() {
    exempt_all(
        "bin/t.dart",
        "#!/usr/bin/env dart\nint a() => 1;\nint b() => 2;\nvoid main() { print(a()); }\n",
    );
}
#[test]
fn perl_exempt() {
    exempt_all(
        "bin/t.pl",
        "#!/usr/bin/perl\nsub a { 1 }\nsub b { 2 }\na();\n",
    );
}
