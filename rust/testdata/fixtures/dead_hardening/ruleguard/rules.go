package gorules

import "github.com/quasilyte/go-ruleguard/dsl"

func revealErrorDropped(m dsl.Matcher) {
	m.Match(`fmt.Errorf($*_)`).Report("error dropped")
}
