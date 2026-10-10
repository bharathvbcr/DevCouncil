package util

// Target02 is called bare from util.go (same package, other file) (C02).
func Target02() int { return 2 }

// DeadUtil has no caller anywhere: the only true-dead function in util.
func DeadUtil() int { return -1 }
