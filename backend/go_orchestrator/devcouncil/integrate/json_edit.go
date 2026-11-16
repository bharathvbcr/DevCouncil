package integrate

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"unicode/utf8"
)

type spanKind uint8

const (
	spanObject spanKind = iota
	spanArray
	spanString
	spanPrimitive
)

type jsonSpan struct {
	kind    spanKind
	start   int
	end     int
	elems   []*jsonSpan
	members []jsonMember
}

type jsonMember struct {
	key   string
	start int // byte offset of key's opening '"'
	end   int // byte offset right after val.end
	val   *jsonSpan
}

func parseJSONSpan(data []byte) (*jsonSpan, error) {
	if !utf8.Valid(data) {
		return nil, errors.New("JSON input is not valid UTF-8")
	}
	pos := skipJSONWS(data, 0)
	if pos >= len(data) {
		return nil, errors.New("empty JSON input")
	}
	span, next, err := scanJSONValue(data, pos, 0)
	if err != nil {
		return nil, err
	}
	if skipJSONWS(data, next) != len(data) {
		return nil, errors.New("unexpected trailing data after JSON value")
	}
	return span, nil
}

func skipJSONWS(data []byte, pos int) int {
	for pos < len(data) {
		switch data[pos] {
		case ' ', '\t', '\r', '\n':
			pos++
		default:
			return pos
		}
	}
	return pos
}

func scanJSONValue(data []byte, pos, depth int) (*jsonSpan, int, error) {
	if depth > 64 {
		return nil, 0, errors.New("JSON nesting exceeds 64 levels")
	}
	if pos >= len(data) {
		return nil, 0, errors.New("unexpected end of JSON")
	}
	switch data[pos] {
	case '{':
		return scanJSONObject(data, pos, depth)
	case '[':
		return scanJSONArray(data, pos, depth)
	case '"':
		end, _, err := scanJSONString(data, pos)
		if err != nil {
			return nil, 0, err
		}
		return &jsonSpan{kind: spanString, start: pos, end: end}, end, nil
	default:
		end := pos
		for end < len(data) {
			switch data[end] {
			case ' ', '\t', '\r', '\n', ',', ']', '}':
				return &jsonSpan{kind: spanPrimitive, start: pos, end: end}, end, nil
			}
			end++
		}
		return &jsonSpan{kind: spanPrimitive, start: pos, end: end}, end, nil
	}
}

func scanJSONString(data []byte, pos int) (int, string, error) {
	if pos >= len(data) || data[pos] != '"' {
		return 0, "", errors.New("expected JSON string")
	}
	i := pos + 1
	escaped := false
	for i < len(data) {
		b := data[i]
		if escaped {
			escaped = false
			i++
			continue
		}
		if b == '\\' {
			escaped = true
			i++
			continue
		}
		if b == '"' {
			end := i + 1
			var decoded string
			if err := json.Unmarshal(data[pos:end], &decoded); err != nil {
				return 0, "", fmt.Errorf("invalid JSON string: %w", err)
			}
			return end, decoded, nil
		}
		i++
	}
	return 0, "", errors.New("unterminated JSON string")
}

func scanJSONObject(data []byte, openPos, depth int) (*jsonSpan, int, error) {
	pos := skipJSONWS(data, openPos+1)
	if pos >= len(data) {
		return nil, 0, errors.New("unterminated JSON object")
	}
	if data[pos] == '}' {
		return &jsonSpan{kind: spanObject, start: openPos, end: pos + 1}, pos + 1, nil
	}
	var members []jsonMember
	for {
		keyStart := pos
		keyEnd, key, err := scanJSONString(data, pos)
		if err != nil {
			return nil, 0, err
		}
		pos = skipJSONWS(data, keyEnd)
		if pos >= len(data) || data[pos] != ':' {
			return nil, 0, errors.New("expected ':' after JSON object key")
		}
		val, valEnd, err := scanJSONValue(data, skipJSONWS(data, pos+1), depth+1)
		if err != nil {
			return nil, 0, err
		}
		members = append(members, jsonMember{key: key, start: keyStart, end: valEnd, val: val})
		pos = skipJSONWS(data, valEnd)
		if pos >= len(data) {
			return nil, 0, errors.New("unterminated JSON object")
		}
		if data[pos] == '}' {
			return &jsonSpan{kind: spanObject, start: openPos, end: pos + 1, members: members}, pos + 1, nil
		}
		if data[pos] != ',' {
			return nil, 0, errors.New("expected ',' or '}' in JSON object")
		}
		pos = skipJSONWS(data, pos+1)
	}
}

func scanJSONArray(data []byte, openPos, depth int) (*jsonSpan, int, error) {
	pos := skipJSONWS(data, openPos+1)
	if pos >= len(data) {
		return nil, 0, errors.New("unterminated JSON array")
	}
	if data[pos] == ']' {
		return &jsonSpan{kind: spanArray, start: openPos, end: pos + 1}, pos + 1, nil
	}
	var elems []*jsonSpan
	for {
		elem, elemEnd, err := scanJSONValue(data, pos, depth+1)
		if err != nil {
			return nil, 0, err
		}
		elems = append(elems, elem)
		pos = skipJSONWS(data, elemEnd)
		if pos >= len(data) {
			return nil, 0, errors.New("unterminated JSON array")
		}
		if data[pos] == ']' {
			return &jsonSpan{kind: spanArray, start: openPos, end: pos + 1, elems: elems}, pos + 1, nil
		}
		if data[pos] != ',' {
			return nil, 0, errors.New("expected ',' or ']' in JSON array")
		}
		pos = skipJSONWS(data, pos+1)
	}
}

type spanItem struct {
	start     int
	end       int
	prefixEnd int // end of unchanged prefix before value replacement (val.start for members, start for elements)
}

// spliceSpanContainer filters and/or replaces items inside one JSON object or
// array while copying untouched items and their surrounding trivia verbatim.
func spliceSpanContainer(
	data []byte,
	container *jsonSpan,
	closeTok byte,
	items []spanItem,
	decide func(index int) (keep bool, replacement []byte, err error),
) ([]byte, int, error) {
	var keptIdx []int
	var rendered [][]byte
	changed := false
	for i, item := range items {
		keep, repl, err := decide(i)
		if err != nil {
			return nil, 0, err
		}
		if !keep {
			changed = true
			continue
		}
		keptIdx = append(keptIdx, i)
		if repl == nil {
			rendered = append(rendered, data[item.start:item.end])
		} else {
			changed = true
			var b bytes.Buffer
			b.Write(data[item.start:item.prefixEnd])
			b.Write(repl)
			rendered = append(rendered, b.Bytes())
		}
	}
	if !changed {
		return data[container.start:container.end], len(keptIdx), nil
	}
	if len(keptIdx) == 0 {
		return []byte{data[container.start], closeTok}, 0, nil
	}
	var buf bytes.Buffer
	buf.Write(data[container.start:items[0].start])
	buf.Write(rendered[0])
	for i := 1; i < len(keptIdx); i++ {
		idx := keptIdx[i]
		// Copy the separator/newline/indent trivia immediately preceding item idx
		// so surviving items retain their own spacing and key/element formatting.
		buf.Write(data[items[idx-1].end:items[idx].start])
		buf.Write(rendered[i])
	}
	buf.Write(data[items[len(items)-1].end:container.end])
	return buf.Bytes(), len(keptIdx), nil
}

func spliceObject(
	data []byte,
	obj *jsonSpan,
	decide func(m jsonMember) (keep bool, newVal []byte, err error),
) ([]byte, int, error) {
	items := make([]spanItem, len(obj.members))
	for i, m := range obj.members {
		items[i] = spanItem{start: m.start, end: m.end, prefixEnd: m.val.start}
	}
	return spliceSpanContainer(data, obj, '}', items, func(i int) (bool, []byte, error) {
		return decide(obj.members[i])
	})
}

func spliceArray(
	data []byte,
	arr *jsonSpan,
	decide func(index int, elem *jsonSpan) (keep bool, newElem []byte, err error),
) ([]byte, int, error) {
	items := make([]spanItem, len(arr.elems))
	for i, e := range arr.elems {
		items[i] = spanItem{start: e.start, end: e.end, prefixEnd: e.start}
	}
	return spliceSpanContainer(data, arr, ']', items, func(i int) (bool, []byte, error) {
		return decide(i, arr.elems[i])
	})
}
