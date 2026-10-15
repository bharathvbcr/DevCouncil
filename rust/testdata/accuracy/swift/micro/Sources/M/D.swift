func fetchCount() -> Int { 0 }
func useParam(fetchCount: () -> Int) -> Int { fetchCount() }
func useClosure() -> Int { let fetchCount = { 3 }; return fetchCount() }
func mark(day: Int) {}
func day() -> Int { 0 }
func callMark() { mark(day: 1) }
enum Unit { case minute }
func f(_ u: Unit) {}
func minute() -> Int { 0 }
func callF() { f(.minute) }
