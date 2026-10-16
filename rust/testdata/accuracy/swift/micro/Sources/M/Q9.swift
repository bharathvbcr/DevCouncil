struct Lot { func mass() -> Int { 1 } }
struct Stock { func mass() -> Int { 2 } }
struct Depot { var lots: [Stock] }
