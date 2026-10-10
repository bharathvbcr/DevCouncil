struct Alpha { func run() -> Int { 1 } }
struct Beta { func run() -> Int { 2 } }
func external() -> Beta { Beta() }   // pretend: returns Beta
func usesParam(_ s: Alpha) -> Int { s.run() }                  // -> Alpha.run (correct)
func usesLocal() -> Int { let s = external(); return s.run() } // -> Beta.run ; DevMap?
func usesLocal2() -> Int { let s: Beta = Beta(); return s.run() } // -> Beta.run
func usesClosure(_ xs: [Beta]) -> Int { xs.map { s in s.run() }.count } // -> Beta.run
