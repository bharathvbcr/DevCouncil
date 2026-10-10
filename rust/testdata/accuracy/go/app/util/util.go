package util

// Target01 is called package-qualified as util.Target01 (C01).
func Target01() int { return 1 }

// Target28 is called through an aliased import u.Target28 (C28).
func Target28() int { return 28 }

// Target02b is called bare from a sibling file in the same package (C02).
func Target02b() int { return Target02() }
