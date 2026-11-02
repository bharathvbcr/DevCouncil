package spec

import (
	"fmt"
	"os"
	"strings"
)

// ValidateSpecification performs static checks across the 7-layer hierarchy.
func ValidateSpecification(spec *Specification, strict bool) (*ValidationResult, error) {
	if spec == nil {
		return nil, fmt.Errorf("specification is nil")
	}

	res := &ValidationResult{
		FeatureID:   spec.FeatureID,
		SpecPath:    spec.SpecPath,
		Valid:       true,
		StoryCount:  len(spec.UserStories),
		TaskCount:   len(spec.Tasks),
		MetricCount: len(spec.SuccessMetrics),
	}

	// 1. Check for unresolved ambiguities: [NEEDS CLARIFICATION]
	if len(spec.Ambiguities) > 0 {
		for _, amb := range spec.Ambiguities {
			res.Findings = append(res.Findings, ValidationFinding{
				Severity: "blocking",
				Rule:     "unresolved_ambiguity",
				File:     spec.SpecPath,
				Message:  fmt.Sprintf("unresolved ambiguity: %s", amb),
			})
		}
	}

	// 2. Check for spec.md existence
	if !fileExists(spec.SpecPath) {
		res.Findings = append(res.Findings, ValidationFinding{
			Severity: "blocking",
			Rule:     "missing_spec_file",
			File:     spec.SpecPath,
			Message:  "spec.md is missing from specification directory",
		})
	}

	// 3. User Story Validation
	if len(spec.UserStories) == 0 {
		res.Findings = append(res.Findings, ValidationFinding{
			Severity: "blocking",
			Rule:     "no_user_stories",
			File:     spec.SpecPath,
			Message:  "specification defines zero user stories; at least one P1 story is required",
		})
	} else {
		hasP1 := false
		for _, us := range spec.UserStories {
			p := strings.ToUpper(strings.TrimSpace(us.Priority))
			if p == "P1" {
				hasP1 = true
			}
			if p != "P1" && p != "P2" && p != "P3" {
				res.Findings = append(res.Findings, ValidationFinding{
					Severity: "advisory",
					Rule:     "invalid_priority",
					File:     spec.SpecPath,
					Message:  fmt.Sprintf("user story %s has unrecognized priority '%s' (expected P1, P2, P3)", us.ID, us.Priority),
				})
			}

			// Validate Gherkin Scenarios
			if len(us.AcceptanceScenarios) == 0 {
				severity := "advisory"
				if strict {
					severity = "blocking"
				}
				res.Findings = append(res.Findings, ValidationFinding{
					Severity: severity,
					Rule:     "missing_acceptance_scenarios",
					File:     spec.SpecPath,
					Message:  fmt.Sprintf("user story %s has no acceptance scenarios (Given/When/Then)", us.ID),
				})
			} else {
				for i, sc := range us.AcceptanceScenarios {
					if sc.Given == "" || sc.When == "" || sc.Then == "" {
						severity := "advisory"
						if strict {
							severity = "blocking"
						}
						res.Findings = append(res.Findings, ValidationFinding{
							Severity: severity,
							Rule:     "incomplete_scenario",
							File:     spec.SpecPath,
							Message:  fmt.Sprintf("user story %s scenario #%d is incomplete: missing Given, When, or Then", us.ID, i+1),
						})
					}
				}
			}
		}

		if !hasP1 {
			res.Findings = append(res.Findings, ValidationFinding{
				Severity: "advisory",
				Rule:     "no_p1_story",
				File:     spec.SpecPath,
				Message:  "specification defines no P1 (MVP) user story",
			})
		}
	}

	// 4. Tasks Validation
	if spec.TasksPath == "" || !fileExists(spec.TasksPath) {
		severity := "advisory"
		if strict {
			severity = "blocking"
		}
		res.Findings = append(res.Findings, ValidationFinding{
			Severity: severity,
			Rule:     "missing_tasks_file",
			File:     spec.TasksPath,
			Message:  "tasks.md is missing from specification directory",
		})
	} else if len(spec.Tasks) == 0 {
		res.Findings = append(res.Findings, ValidationFinding{
			Severity: "advisory",
			Rule:     "empty_tasks",
			File:     spec.TasksPath,
			Message:  "tasks.md contains zero executable task entries",
		})
	}

	// 5. Plan & Constitution Checks (Article VII: Simplicity Limit)
	if spec.PlanPath != "" && fileExists(spec.PlanPath) {
		planContent, err := os.ReadFile(spec.PlanPath)
		if err == nil {
			text := string(planContent)
			if strings.Contains(strings.ToLower(text), "complexity tracking") {
				spec.Constitution.ArticlesRead = append(spec.Constitution.ArticlesRead, "Article VII: Simplicity Limit")
			}
		}
	} else {
		res.Findings = append(res.Findings, ValidationFinding{
			Severity: "advisory",
			Rule:     "missing_plan_file",
			File:     spec.PlanPath,
			Message:  "plan.md is missing from specification directory",
		})
	}

	// Set overall validity: valid if zero blocking findings
	for _, f := range res.Findings {
		if f.Severity == "blocking" {
			res.Valid = false
			break
		}
	}

	return res, nil
}
