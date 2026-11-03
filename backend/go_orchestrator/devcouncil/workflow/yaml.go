package workflow

import (
	"bufio"
	"encoding/json"
	"fmt"
	"os"
	"strconv"
	"strings"
)

// ParseWorkflow reads a workflow definition from a YAML or JSON file.
func ParseWorkflow(path string) (*WorkflowDefinition, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("failed to read workflow file %s: %w", path, err)
	}

	content := strings.TrimSpace(string(data))
	// If it starts with '{', treat as JSON directly
	if strings.HasPrefix(content, "{") {
		var def WorkflowDefinition
		if err := json.Unmarshal(data, &def); err != nil {
			return nil, fmt.Errorf("invalid workflow JSON: %w", err)
		}
		return &def, nil
	}

	// Otherwise parse line-oriented YAML
	return parseSimpleYAML(content)
}

func parseSimpleYAML(content string) (*WorkflowDefinition, error) {
	def := &WorkflowDefinition{
		Variables: make(map[string]string),
	}

	scanner := bufio.NewScanner(strings.NewReader(content))
	var section string // "variables", "steps", or top-level
	var currentStep *StepDefinition
	var inAllowedDecisions bool

	for scanner.Scan() {
		raw := scanner.Text()
		trimmed := strings.TrimSpace(raw)
		if trimmed == "" || strings.HasPrefix(trimmed, "#") {
			continue
		}

		indent := len(raw) - len(strings.TrimLeft(raw, " "))

		// Top level keys (indent 0)
		if indent == 0 {
			inAllowedDecisions = false
			parts := strings.SplitN(trimmed, ":", 2)
			key := strings.TrimSpace(parts[0])
			val := ""
			if len(parts) > 1 {
				val = strings.TrimSpace(parts[1])
				val = strings.Trim(val, `"'`)
			}

			switch key {
			case "name":
				def.Name = unquote(val)
				section = "name"
			case "description":
				def.Description = unquote(val)
				section = "description"
			case "variables":
				section = "variables"
			case "steps":
				section = "steps"
			default:
				section = key
			}
			continue
		}

		// Inside variables section
		if section == "variables" && indent > 0 && !strings.HasPrefix(trimmed, "-") {
			parts := strings.SplitN(trimmed, ":", 2)
			if len(parts) == 2 {
				k := strings.TrimSpace(parts[0])
				v := unquote(parts[1])
				def.Variables[k] = v
			}
			continue
		}

		// Inside steps section
		if section == "steps" {
			if inAllowedDecisions && strings.HasPrefix(trimmed, "- ") && !strings.Contains(trimmed, ":") {
				item := strings.Trim(strings.TrimSpace(trimmed[2:]), `"'`)
				if currentStep != nil {
					currentStep.AllowedDecisions = append(currentStep.AllowedDecisions, item)
				}
				continue
			}

			if strings.HasPrefix(trimmed, "- ") {
				// New step entry
				if currentStep != nil {
					def.Steps = append(def.Steps, *currentStep)
				}
				currentStep = &StepDefinition{
					Inputs: make(map[string]string),
				}
				inAllowedDecisions = false
				itemRest := strings.TrimSpace(trimmed[2:])
				if itemRest != "" && strings.Contains(itemRest, ":") {
					parts := strings.SplitN(itemRest, ":", 2)
					assignStepField(currentStep, strings.TrimSpace(parts[0]), strings.TrimSpace(parts[1]))
				}
				continue
			}

			if currentStep != nil {

				parts := strings.SplitN(trimmed, ":", 2)
				if len(parts) == 2 {
					k := strings.TrimSpace(parts[0])
					v := strings.TrimSpace(parts[1])
					if k == "allowed_decisions" {
						inAllowedDecisions = true
						if v != "" && strings.HasPrefix(v, "[") && strings.HasSuffix(v, "]") {
							// Inline list: [approve, reject]
							inner := strings.Trim(v, "[]")
							for _, opt := range strings.Split(inner, ",") {
								optClean := strings.Trim(strings.TrimSpace(opt), `"'`)
								if optClean != "" {
									currentStep.AllowedDecisions = append(currentStep.AllowedDecisions, optClean)
								}
							}
							inAllowedDecisions = false
						}
						continue
					}
					inAllowedDecisions = false
					assignStepField(currentStep, k, v)
				}
			}
		}
	}

	if currentStep != nil {
		def.Steps = append(def.Steps, *currentStep)
	}

	if err := scanner.Err(); err != nil {
		return nil, err
	}

	if def.Name == "" {
		def.Name = "unnamed-workflow"
	}

	return def, nil
}

func unquote(s string) string {
	s = strings.TrimSpace(s)
	if len(s) >= 2 {
		if (strings.HasPrefix(s, `"`) && strings.HasSuffix(s, `"`)) || (strings.HasPrefix(s, `'`) && strings.HasSuffix(s, `'`)) {
			return s[1 : len(s)-1]
		}
	}
	return s
}

func assignStepField(step *StepDefinition, key, val string) {
	val = unquote(val)
	switch key {
	case "id":
		step.ID = val
	case "type":
		step.Type = StepType(val)
	case "description":
		step.Description = val
	case "command":
		step.Command = val
	case "prompt":
		step.Prompt = val
	case "condition":
		step.Condition = val
	case "then_step":
		step.ThenStep = val
	case "else_step":
		step.ElseStep = val
	case "max_iterations":
		if n, err := strconv.Atoi(val); err == nil {
			step.MaxIterations = n
		}
	case "depends_on":
		raw := strings.Trim(val, "[]")
		var deps []string
		for _, part := range strings.Split(raw, ",") {
			p := strings.TrimSpace(part)
			if p != "" {
				deps = append(deps, unquote(p))
			}
		}
		step.DependsOn = deps
	default:
		if step.Inputs == nil {
			step.Inputs = make(map[string]string)
		}
		step.Inputs[key] = val
	}
}
