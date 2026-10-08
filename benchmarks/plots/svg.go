package main

import (
	"fmt"
	"html"
	"math"
	"strings"
)

// Series is one colored bar or line in a chart.
type Series struct {
	Name  string
	Color string
}

// Cell is one measurement. V is the headline value (the minimum for the
// interleaved timing benchmarks, the mean for score charts). Med and Max are
// optional: NaN means "not recorded" and nothing is drawn for them.
type Cell struct{ V, Med, Max float64 }

var absent = math.NaN()

func single(v float64) Cell { return Cell{v, absent, absent} }

func missing() Cell { return Cell{absent, absent, absent} }

func (c Cell) has() bool { return !math.IsNaN(c.V) }

// BarOpts describes a grouped bar chart. Cells is indexed [group][series].
type BarOpts struct {
	Title, Sub string
	YLabel     string
	Groups     []string
	Series     []Series
	Cells      [][]Cell
	Log        bool
	YMax       float64
	Fmt        func(float64) string
	Labels     bool
}

// LineOpts describes a line chart. Vals is indexed [series][x]; NaN breaks the line.
type LineOpts struct {
	Title, Sub string
	YLabel     string
	XLabels    []string
	Series     []Series
	Vals       [][]float64
	Log        bool
	Fmt        func(float64) string
}

const (
	chartW  = 980
	plotH   = 300
	marginL = 76
	marginR = 24
	marginT = 84
)

const svgStyle = `<style>
.t{fill:#1f2328;font:12px -apple-system,Segoe UI,Helvetica,Arial,sans-serif}
.m{fill:#59636e;font:11px -apple-system,Segoe UI,Helvetica,Arial,sans-serif}
.h{fill:#1f2328;font:600 15px -apple-system,Segoe UI,Helvetica,Arial,sans-serif}
.bg{fill:#ffffff}
.g{stroke:#d1d9e0;stroke-width:1}
.ax{stroke:#8c959f;stroke-width:1}
.w{stroke:#1f2328;stroke-width:1.2;fill:none}
.k{stroke:#1f2328;stroke-width:2.2}
@media (prefers-color-scheme: dark){
.bg{fill:#0d1117}.t,.h{fill:#e6edf3}.m{fill:#9198a1}.g{stroke:#30363d}.ax{stroke:#6e7681}.w{stroke:#e6edf3}.k{stroke:#e6edf3}
}
</style>`

func esc(s string) string { return html.EscapeString(s) }

func fmtNum(v float64) string {
	switch {
	case v >= 1000:
		return fmt.Sprintf("%.0f", v)
	case v >= 100:
		return fmt.Sprintf("%.0f", v)
	case v >= 10:
		return fmt.Sprintf("%.1f", v)
	case v >= 1:
		return fmt.Sprintf("%.2f", v)
	default:
		return fmt.Sprintf("%.3g", v)
	}
}

func fmtPct(v float64) string { return fmt.Sprintf("%.0f%%", v*100) }

// labelPad returns the extra left room, in pixels, that the rotated x labels
// need so the leftmost one is not clipped. Labels are anchored at their end, so
// a long label under the first group reaches left past the axis.
func labelPad(labels []string, groupW float64) int {
	pad := 0.0
	for i, l := range labels {
		left := float64(marginL) + groupW*(float64(i)+0.5) + 6 - float64(len([]rune(l)))*7.2*0.85
		pad = math.Max(pad, 12-left)
	}
	return int(math.Ceil(pad))
}

// labelDrop returns the height, in pixels, the rotated x labels need below the
// axis: the longest label hangs down-left at the rotation angle.
func labelDrop(labels []string) int {
	longest := 0
	for _, l := range labels {
		longest = max(longest, len([]rune(l)))
	}
	return max(60, int(math.Ceil(float64(longest)*7.2*0.53))+28)
}

// header opens the svg and a group shifted right by pad, so every drawing
// call after it works in the unpadded coordinate system. Callers close with
// "</g></svg>".
func header(b *strings.Builder, pad, h int, title, sub string, series []Series) {
	w := chartW + pad
	fmt.Fprintf(b, `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 %d %d" width="%d" height="%d" role="img" aria-label="%s">`, w, h, w, h, esc(title))
	fmt.Fprintf(b, `<title>%s</title>%s<rect class="bg" width="%d" height="%d"/><g transform="translate(%d 0)">`, esc(title), svgStyle, w, h, pad)
	fmt.Fprintf(b, `<text class="h" x="%d" y="26">%s</text>`, marginL, esc(title))
	if sub != "" {
		fmt.Fprintf(b, `<text class="m" x="%d" y="46">%s</text>`, marginL, esc(sub))
	}
	x := float64(marginL)
	for _, s := range series {
		fmt.Fprintf(b, `<rect x="%.1f" y="58" width="10" height="10" rx="2" fill="%s"/>`, x, s.Color)
		fmt.Fprintf(b, `<text class="t" x="%.1f" y="67">%s</text>`, x+14, esc(s.Name))
		x += 14 + float64(len(s.Name))*6.6 + 18
	}
}

type axis struct {
	log    bool
	lo, hi float64
	top    float64
	height float64
}

func (a axis) y(v float64) float64 {
	if a.log {
		if v < a.lo {
			v = a.lo
		}
		f := (math.Log10(v) - math.Log10(a.lo)) / (math.Log10(a.hi) - math.Log10(a.lo))
		return a.top + a.height - f*a.height
	}
	return a.top + a.height - (v-a.lo)/(a.hi-a.lo)*a.height
}

func niceCeil(v float64) float64 {
	if v <= 0 {
		return 1
	}
	exp := math.Pow(10, math.Floor(math.Log10(v)))
	for _, m := range []float64{1, 1.5, 2, 2.5, 3, 4, 5, 6, 8, 10} {
		if m*exp >= v {
			return m * exp
		}
	}
	return 10 * exp
}

func newAxis(log bool, minPos, maxV, forceMax float64) axis {
	a := axis{log: log, top: marginT, height: plotH}
	if log {
		if math.IsInf(minPos, 1) {
			minPos = 1
		}
		a.lo = math.Pow(10, math.Floor(math.Log10(minPos*0.9)))
		a.hi = math.Pow(10, math.Ceil(math.Log10(maxV*1.05)))
		if a.hi <= a.lo {
			a.hi = a.lo * 10
		}
		return a
	}
	a.lo = 0
	a.hi = niceCeil(maxV * 1.05)
	if forceMax > 0 {
		a.hi = forceMax
	}
	return a
}

func drawAxis(b *strings.Builder, a axis, f func(float64) string, ylabel string, plotW float64) {
	if a.log {
		for e := math.Log10(a.lo); e <= math.Log10(a.hi)+1e-9; e++ {
			v := math.Pow(10, e)
			y := a.y(v)
			fmt.Fprintf(b, `<line class="g" x1="%d" y1="%.1f" x2="%.1f" y2="%.1f"/><text class="m" x="%d" y="%.1f" text-anchor="end">%s</text>`, marginL, y, float64(marginL)+plotW, y, marginL-6, y+4, f(v))
		}
	} else {
		for i := 0; i <= 5; i++ {
			v := a.lo + (a.hi-a.lo)*float64(i)/5
			y := a.y(v)
			fmt.Fprintf(b, `<line class="g" x1="%d" y1="%.1f" x2="%.1f" y2="%.1f"/><text class="m" x="%d" y="%.1f" text-anchor="end">%s</text>`, marginL, y, float64(marginL)+plotW, y, marginL-6, y+4, f(v))
		}
	}
	fmt.Fprintf(b, `<line class="ax" x1="%d" y1="%.1f" x2="%.1f" y2="%.1f"/>`, marginL, a.y(a.lo), float64(marginL)+plotW, a.y(a.lo))
	if ylabel != "" {
		fmt.Fprintf(b, `<text class="m" transform="translate(14 %.1f) rotate(-90)" text-anchor="middle">%s</text>`, a.top+a.height/2, esc(ylabel))
	}
}

func xLabel(b *strings.Builder, x, y float64, s string) {
	fmt.Fprintf(b, `<text class="t" transform="translate(%.1f %.1f) rotate(-32)" text-anchor="end">%s</text>`, x, y, esc(s))
}

// barChart renders a grouped bar chart. The bar is the headline value; when a
// cell records Med and Max, a tick marks the median and a whisker reaches the max.
func barChart(o BarOpts) string {
	if o.Fmt == nil {
		o.Fmt = fmtNum
	}
	maxV, minPos := 0.0, math.Inf(1)
	for _, row := range o.Cells {
		for _, c := range row {
			for _, v := range []float64{c.V, c.Med, c.Max} {
				if math.IsNaN(v) {
					continue
				}
				maxV = math.Max(maxV, v)
				if v > 0 {
					minPos = math.Min(minPos, v)
				}
			}
		}
	}
	a := newAxis(o.Log, minPos, maxV, o.YMax)
	plotW := float64(chartW - marginL - marginR)
	var b strings.Builder
	header(&b, labelPad(o.Groups, plotW/float64(len(o.Groups))), marginT+plotH+labelDrop(o.Groups), o.Title, o.Sub, o.Series)
	drawAxis(&b, a, o.Fmt, o.YLabel, plotW)
	gw := plotW / float64(len(o.Groups))
	inner := gw * 0.8
	bw := math.Min(inner/float64(len(o.Series)), 30)
	base := a.y(a.lo)
	for gi, g := range o.Groups {
		gx := float64(marginL) + gw*float64(gi)
		start := gx + (gw-bw*float64(len(o.Series)))/2
		for si, s := range o.Series {
			c := o.Cells[gi][si]
			if !c.has() {
				continue
			}
			x := start + bw*float64(si)
			y := a.y(c.V)
			tip := fmt.Sprintf("%s / %s: %s", g, s.Name, o.Fmt(c.V))
			if !math.IsNaN(c.Med) {
				tip += fmt.Sprintf(" (median %s", o.Fmt(c.Med))
				if !math.IsNaN(c.Max) {
					tip += fmt.Sprintf(", max %s", o.Fmt(c.Max))
				}
				tip += ")"
			}
			fmt.Fprintf(&b, `<rect x="%.1f" y="%.1f" width="%.1f" height="%.1f" rx="2" fill="%s"><title>%s</title></rect>`, x+1, y, bw-2, math.Max(base-y, 1), s.Color, esc(tip))
			cx := x + bw/2
			if !math.IsNaN(c.Max) && c.Max > c.V {
				fmt.Fprintf(&b, `<path class="w" d="M%.1f %.1fV%.1fM%.1f %.1fh6"/>`, cx, y, a.y(c.Max), cx-3, a.y(c.Max))
			}
			if !math.IsNaN(c.Med) {
				fmt.Fprintf(&b, `<line class="k" x1="%.1f" y1="%.1f" x2="%.1f" y2="%.1f"/>`, x+2, a.y(c.Med), x+bw-2, a.y(c.Med))
			}
			if o.Labels && c.V != 0 {
				top := y
				if !math.IsNaN(c.Max) && c.Max > c.V {
					top = a.y(c.Max)
				}
				fmt.Fprintf(&b, `<text class="m" x="%.1f" y="%.1f" text-anchor="middle" font-size="10">%s</text>`, cx, top-4, o.Fmt(c.V))
			}
		}
		xLabel(&b, gx+gw/2+6, float64(marginT+plotH)+16, g)
	}
	b.WriteString("</g></svg>\n")
	return b.String()
}

func lineChart(o LineOpts) string {
	if o.Fmt == nil {
		o.Fmt = fmtNum
	}
	maxV, minPos := 0.0, math.Inf(1)
	for _, row := range o.Vals {
		for _, v := range row {
			if math.IsNaN(v) {
				continue
			}
			maxV = math.Max(maxV, v)
			if v > 0 {
				minPos = math.Min(minPos, v)
			}
		}
	}
	a := newAxis(o.Log, minPos, maxV, 0)
	plotW := float64(chartW - marginL - marginR)
	var b strings.Builder
	header(&b, labelPad(o.XLabels, plotW/float64(len(o.XLabels))), marginT+plotH+labelDrop(o.XLabels), o.Title, o.Sub, o.Series)
	drawAxis(&b, a, o.Fmt, o.YLabel, plotW)
	step := plotW / float64(len(o.XLabels))
	xs := func(i int) float64 { return float64(marginL) + step*(float64(i)+0.5) }
	for si, s := range o.Series {
		var path strings.Builder
		pen := false
		for i, v := range o.Vals[si] {
			if math.IsNaN(v) {
				pen = false
				continue
			}
			cmd := "L"
			if !pen {
				cmd = "M"
				pen = true
			}
			fmt.Fprintf(&path, "%s%.1f %.1f", cmd, xs(i), a.y(v))
		}
		fmt.Fprintf(&b, `<path d="%s" fill="none" stroke="%s" stroke-width="2" stroke-linejoin="round" stroke-linecap="round"/>`, path.String(), s.Color)
		for i, v := range o.Vals[si] {
			if math.IsNaN(v) {
				continue
			}
			fmt.Fprintf(&b, `<circle cx="%.1f" cy="%.1f" r="3.5" fill="%s"><title>%s / %s: %s</title></circle>`, xs(i), a.y(v), s.Color, esc(o.XLabels[i]), esc(s.Name), o.Fmt(v))
		}
	}
	for i, l := range o.XLabels {
		xLabel(&b, xs(i)+6, float64(marginT+plotH)+16, l)
	}
	b.WriteString("</g></svg>\n")
	return b.String()
}
