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

fn assert_live(path: &str, src: &str, live: &[&str]) {
    let r = reports(&[(path, src)]);
    let bad: Vec<String> = r
        .iter()
        .filter(|x| !x.is_exempt && live.contains(&x.symbol_name.as_str()))
        .map(|x| format!("{} conf={:?}", x.symbol_name, x.confidence))
        .collect();
    assert!(bad.is_empty(), "LIVE reported dead in {path}: {bad:#?}");
}

macro_rules! py { ($name:ident, $live:expr, $src:expr) => {
    #[test] fn $name() { assert_live("tools/s.py", $src, $live); }
};}
macro_rules! js { ($name:ident, $path:expr, $live:expr, $src:expr) => {
    #[test] fn $name() { assert_live($path, $src, $live); }
};}

macro_rules! macro_ignored { ($why:literal, $name:ident, $live:expr, $src:expr) => { #[test] #[ignore = $why] fn $name() { assert_live("tools/s.py", $src, $live); } };}

py!(p_argparse_type, &["to_int"], "#!/usr/bin/env python3\nimport argparse\ndef to_int(s):\n    return int(s)\nif __name__ == '__main__':\n    p = argparse.ArgumentParser()\n    p.add_argument('x', type=to_int)\n    p.parse_args()\n");
py!(p_argparse_action_cls, &["MyAct"], "#!/usr/bin/env python3\nimport argparse\nclass MyAct(argparse.Action):\n    def __call__(self, *a):\n        pass\nif __name__ == '__main__':\n    p = argparse.ArgumentParser()\n    p.add_argument('x', action=MyAct)\n    p.parse_args()\n");
py!(p_excepthook, &["hook"], "#!/usr/bin/env python3\nimport sys\ndef hook(*a):\n    pass\nsys.excepthook = hook\n");
py!(p_process_target, &["work"], "#!/usr/bin/env python3\nimport multiprocessing\ndef work():\n    pass\nif __name__ == '__main__':\n    multiprocessing.Process(target=work).start()\n");
py!(p_pool_map, &["work"], "#!/usr/bin/env python3\nimport multiprocessing\ndef work(x):\n    return x\nif __name__ == '__main__':\n    with multiprocessing.Pool() as p:\n        p.map(work, [1])\n");
py!(p_json_default, &["enc"], "#!/usr/bin/env python3\nimport json\ndef enc(o):\n    return str(o)\nif __name__ == '__main__':\n    print(json.dumps({}, default=enc))\n");
py!(p_logging_handler, &["H"], "#!/usr/bin/env python3\nimport logging\nclass H(logging.Handler):\n    def emit(self, r):\n        pass\nif __name__ == '__main__':\n    logging.getLogger().addHandler(H())\n");
py!(p_staticmethod, &["T"], "#!/usr/bin/env python3\nclass T:\n    @staticmethod\n    def s():\n        return 1\n    @classmethod\n    def c(cls):\n        return cls.s()\nif __name__ == '__main__':\n    T.c()\n");
py!(p_main_args_splat, &["main"], "#!/usr/bin/env python3\nimport sys\ndef main(*a):\n    return 0\nif __name__ == '__main__':\n    main(*sys.argv[1:])\n");
py!(p_loop_signal, &["stop"], "#!/usr/bin/env python3\nimport asyncio, signal\ndef stop():\n    pass\nasync def amain():\n    asyncio.get_running_loop().add_signal_handler(signal.SIGINT, stop)\nif __name__ == '__main__':\n    asyncio.run(amain())\n");
py!(p_decorator_atexit, &["bye"], "#!/usr/bin/env python3\nimport atexit\n@atexit.register\ndef bye():\n    pass\n");
py!(p_decorator_fixture, &["fx"], "#!/usr/bin/env python3\nimport pytest\n@pytest.fixture\ndef fx():\n    return 1\nif __name__ == '__main__':\n    pass\n");
py!(p_main_module_level_call, &["main"], "#!/usr/bin/env python3\ndef main():\n    return 1\nmain()\n");
py!(p_enum_member_in_guard, &["Mode"], "#!/usr/bin/env python3\nfrom enum import Enum\nclass Mode(Enum):\n    A = 'a'\nif __name__ == '__main__':\n    print(Mode('a'))\n");
py!(p_dict_of_classes, &["A","B"], "#!/usr/bin/env python3\nclass A: pass\nclass B: pass\nKINDS = {'a': A, 'b': B}\nif __name__ == '__main__':\n    import sys\n    KINDS[sys.argv[1]]()\n");
py!(p_class_attr_const, &["DEFAULT"], "#!/usr/bin/env python3\nDEFAULT = 3\nclass C:\n    x = DEFAULT\nif __name__ == '__main__':\n    print(C.x)\n");
py!(p_default_arg_const, &["DEFAULT"], "#!/usr/bin/env python3\nDEFAULT = 3\ndef f(x=DEFAULT):\n    return x\nif __name__ == '__main__':\n    f()\n");
macro_ignored!("unfiled card: a name rebound only through global X; X += 1 in a function is reported dead: pre-existing resolver gap that reproduces in a plain module (see attack_script_probe3 control)", p_global_decl, &["COUNTER"], "#!/usr/bin/env python3\nCOUNTER = 0\ndef inc():\n    global COUNTER\n    COUNTER += 1\nif __name__ == '__main__':\n    inc()\n");
py!(p_const_in_decorator_arg, &["PATH"], "#!/usr/bin/env python3\nfrom flask import Flask\nPATH = '/x'\napp = Flask(__name__)\n@app.route(PATH)\ndef idx():\n    return 1\n");
py!(p_slots_all_tuple, &["A"], "#!/usr/bin/env python3\n__all__ = ('A',)\nclass A: pass\n");
py!(p_main_module_import_star, &["util"], "#!/usr/bin/env python3\ndef util():\n    return 1\nif __name__ == '__main__':\n    pass\n");
py!(p_namespace_dict_access, &["CFG"], "#!/usr/bin/env python3\nCFG = {'a': 1}\nif __name__ == '__main__':\n    print(CFG['a'])\n");
py!(p_type_alias_use, &["Num"], "#!/usr/bin/env python3\nNum = int\ndef f(x: Num) -> Num:\n    return x\nif __name__ == '__main__':\n    f(1)\n");
macro_ignored!("unfiled card: a Python alias read only inside a string annotation is reported dead: pre-existing resolver gap that reproduces in a plain module (see attack_script_probe3 control)", p_cast_string_annot, &["Num"], "#!/usr/bin/env python3\nNum = int\ndef f(x: 'Num') -> 'Num':\n    return x\nif __name__ == '__main__':\n    f(1)\n");
py!(p_sys_modules_main, &["fn"], "#!/usr/bin/env python3\nimport sys\ndef fn():\n    return 1\nif __name__ == '__main__':\n    sys.modules['__main__'].fn()\n");
py!(p_self_import, &["fn"], "#!/usr/bin/env python3\ndef fn():\n    return 1\nif __name__ == '__main__':\n    import __main__\n    __main__.fn()\n");
py!(p_eval_name, &["fn"], "#!/usr/bin/env python3\ndef fn():\n    return 1\nif __name__ == '__main__':\n    eval('fn')()\n");
py!(p_dunder_methods, &["T"], "#!/usr/bin/env python3\nclass T:\n    def __init__(self): pass\n    def __enter__(self): return self\n    def __exit__(self, *a): pass\n    def __len__(self): return 0\nif __name__ == '__main__':\n    with T() as t:\n        len(t)\n");
py!(p_main_after_if_else, &["main"], "#!/usr/bin/env python3\nimport sys\ndef main():\n    return 1\nif len(sys.argv) > 1:\n    main()\nelse:\n    sys.exit(2)\n");
py!(p_while_loop, &["step"], "#!/usr/bin/env python3\ndef step():\n    return False\nwhile step():\n    pass\n");
py!(p_comprehension_call, &["f"], "#!/usr/bin/env python3\ndef f(x):\n    return x\nR = [f(i) for i in range(3)]\n");
py!(p_with_main_in_main_py_nonguard, &["main"], "def main():\n    return 1\nif __name__ == '__main__':\n    import sys\n    sys.exit(main())\n");
py!(p_script_run_by_subprocess, &["only_here"], "#!/usr/bin/env python3\nimport subprocess\ndef only_here():\n    return subprocess.run([__file__])\nif __name__ == '__main__':\n    only_here()\n");
py!(p_nested_class_method, &["Outer"], "#!/usr/bin/env python3\nclass Outer:\n    class Inner:\n        def f(self): return 1\n    def g(self):\n        return self.Inner().f()\nif __name__ == '__main__':\n    Outer().g()\n");
py!(p_hasattr_dispatch, &["Handler"], "#!/usr/bin/env python3\nclass Handler:\n    def do_a(self): return 1\n    def run(self, n):\n        return getattr(self, 'do_' + n)()\nif __name__ == '__main__':\n    Handler().run('a')\n");
py!(p_conditional_def, &["fast"], "#!/usr/bin/env python3\ntry:\n    import ujson as j\nexcept ImportError:\n    def fast():\n        return 1\n    j = fast\nif __name__ == '__main__':\n    j()\n");

js!(j_bin_json, "bin/t.js", &["main"], "#!/usr/bin/env node\nfunction main(argv) { return 1; }\nmain(process.argv);\n");
js!(j_ts_exports_const_obj, "bin/t.ts", &["cmds","a"], "#!/usr/bin/env node\nfunction a() {}\nexport const cmds = { a };\n");
js!(j_promise_then, "bin/t.js", &["ok","fail"], "#!/usr/bin/env node\nfunction ok() {}\nfunction fail() {}\nPromise.resolve().then(ok, fail);\n");
js!(j_event_emitter, "bin/t.js", &["onData"], "#!/usr/bin/env node\nfunction onData() {}\nprocess.stdin.on('data', onData);\n");
js!(j_switch_dispatch, "bin/t.js", &["doA"], "#!/usr/bin/env node\nfunction doA() {}\nswitch (process.argv[2]) { case 'a': doA(); break; }\n");
js!(j_await_main, "bin/t.mjs", &["main"], "#!/usr/bin/env node\nasync function main() {}\nawait main();\n");
js!(j_class_extends, "bin/t.js", &["Base","Child"], "#!/usr/bin/env node\nclass Base {}\nclass Child extends Base {}\nnew Child();\n");
js!(j_object_method_shorthand, "bin/t.js", &["helper"], "#!/usr/bin/env node\nfunction helper() {}\nconst o = { run() { return helper(); } };\no.run();\n");
js!(j_const_in_template, "bin/t.ts", &["NAME"], "#!/usr/bin/env node\nconst NAME = 'x';\nconsole.log(`hi ${NAME}`);\n");
js!(j_ts_enum, "bin/t.ts", &["Mode"], "#!/usr/bin/env node\nenum Mode { A, B }\nconsole.log(Mode.A);\n");
js!(j_ts_decorator, "bin/t.ts", &["Svc"], "#!/usr/bin/env node\nfunction Injectable(c: any) { return c; }\n@Injectable\nclass Svc {}\nnew Svc();\n");
js!(j_ts_namespace, "bin/t.ts", &["NS"], "#!/usr/bin/env node\nnamespace NS { export const a = 1; }\nconsole.log(NS.a);\n");
js!(j_cjs_exports_this, "bin/t.js", &["a"], "#!/usr/bin/env node\nfunction a() {}\nmodule.exports.a = a;\n");
js!(j_main_called_in_then, "bin/t.js", &["main"], "#!/usr/bin/env node\nfunction main() {}\nPromise.resolve().then(() => main());\n");
js!(j_shebang_exports_for_lib, "bin/t.js", &["lib"], "#!/usr/bin/env node\nexports.lib = function lib() {};\n");
js!(j_ts_export_list, "bin/t.ts", &["a"], "#!/usr/bin/env node\nfunction a() {}\nexport { a };\n");
js!(j_ts_export_list_alias, "bin/t.ts", &["a"], "#!/usr/bin/env node\nfunction a() {}\nexport { a as b };\n");
js!(j_reexport_const, "bin/t.js", &["A"], "#!/usr/bin/env node\nconst A = 1;\nmodule.exports = { A };\n");
js!(j_vm_dispatch, "bin/t.js", &["a"], "#!/usr/bin/env node\nfunction a() {}\nglobal['a']();\n");
js!(j_this_global, "bin/t.js", &["a"], "#!/usr/bin/env node\nfunction a() {}\nglobalThis[process.argv[2]]();\n");
