package spec

import (
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"regexp"
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

	// 6. Unedited scaffold text from `spec init`
	placeholderFindings, err := scaffoldPlaceholderFindings(spec, strict)
	if err != nil {
		return nil, err
	}
	res.Findings = append(res.Findings, placeholderFindings...)

	// Set overall validity: valid if zero blocking findings
	for _, f := range res.Findings {
		if f.Severity == "blocking" {
			res.Valid = false
			break
		}
	}

	return res, nil
}

// scaffoldPlaceholderFindings reports spec.md, plan.md and tasks.md files that
// still carry the placeholder text ScaffoldFeature writes: one finding per
// file, at the line of its earliest placeholder. Blocking under strict,
// advisory otherwise. A file that exists but cannot be read is an error, not
// a pass.
func scaffoldPlaceholderFindings(spec *Specification, strict bool) ([]ValidationFinding, error) {
	severity := "advisory"
	if strict {
		severity = "blocking"
	}
	var out []ValidationFinding
	for _, f := range []struct{ name, path string }{
		{"spec.md", spec.SpecPath},
		{"plan.md", spec.PlanPath},
		{"tasks.md", spec.TasksPath},
	} {
		if f.path == "" || !fileExists(f.path) {
			continue
		}
		raw, err := os.ReadFile(f.path)
		if err != nil {
			return nil, fmt.Errorf("read %s: %w", f.path, err)
		}
		text := string(raw)
		count, first, firstIdx := 0, "", -1
		for _, marker := range scaffoldPlaceholders[f.name] {
			idx := strings.Index(text, marker)
			if idx < 0 {
				continue
			}
			count++
			if firstIdx < 0 || idx < firstIdx {
				first, firstIdx = marker, idx
			}
		}
		if count == 0 {
			continue
		}
		out = append(out, ValidationFinding{
			Severity: severity,
			Rule:     "scaffold_placeholder",
			File:     f.path,
			Line:     strings.Count(text[:firstIdx], "\n") + 1,
			Message:  fmt.Sprintf("%s still contains %d unedited `spec init` placeholder(s), first %q; replace them with the feature's real content", f.name, count, first),
		})
	}
	return out, nil
}

// reSpecDirName matches the directories `spec validate --all` checks:
// specs/NNN-<name>. Other entries under specs/ (READMEs, a Lean project,
// NNN_<name>) are not specifications and are left alone.
var reSpecDirName = regexp.MustCompile(`^\d{3}-`)

// ValidateSpecDir parses and validates the specification in specDir. Both the
// single-spec and the repository-wide form of `spec validate` go through it.
func ValidateSpecDir(specDir string, strict bool) (*ValidationResult, error) {
	parsed, err := ParseSpecification(specDir)
	if err != nil {
		return nil, fmt.Errorf("failed to parse: %w", err)
	}
	return ValidateSpecification(parsed, strict)
}

// ListSpecDirs returns <projectRoot>/specs/NNN-* directories in name order.
// A missing specs/ or one holding no NNN- directory is an error: a gate that
// found nothing to check must not read as a pass. A symlink whose target
// cannot be read is kept, so validating it fails rather than skipping it.
func ListSpecDirs(projectRoot string) ([]string, error) {
	specsRoot := filepath.Join(projectRoot, "specs")
	entries, err := os.ReadDir(specsRoot)
	if err != nil {
		return nil, fmt.Errorf("cannot read specs directory %s: %w", specsRoot, err)
	}
	var dirs []string
	for _, e := range entries {
		if !reSpecDirName.MatchString(e.Name()) {
			continue
		}
		p := filepath.Join(specsRoot, e.Name())
		switch {
		case e.IsDir():
			dirs = append(dirs, p)
		case e.Type()&fs.ModeSymlink != 0:
			if fi, err := os.Stat(p); err != nil || fi.IsDir() {
				dirs = append(dirs, p)
			}
		}
	}
	if len(dirs) == 0 {
		return nil, fmt.Errorf("%s contains no NNN- spec directories (e.g. 001-feature); nothing to validate", specsRoot)
	}
	return dirs, nil
}

// ValidateAllSpecifications validates every specs/NNN-* directory under
// projectRoot. A specification that cannot be parsed is reported as invalid
// with a blocking validation_error finding, never skipped. The returned error
// is non-nil only when there was nothing to validate; the result then carries
// the same message in Error and Valid is false.
func ValidateAllSpecifications(projectRoot string, strict bool) (*SuiteValidationResult, error) {
	res := &SuiteValidationResult{
		SpecsRoot: filepath.Join(projectRoot, "specs"),
		Strict:    strict,
		Specs:     []ValidationResult{},
	}
	dirs, err := ListSpecDirs(projectRoot)
	if err != nil {
		res.Error = err.Error()
		return res, err
	}
	for _, dir := range dirs {
		r, err := ValidateSpecDir(dir, strict)
		if err != nil {
			r = &ValidationResult{
				FeatureID: filepath.Base(dir),
				SpecPath:  filepath.Join(dir, "spec.md"),
				Findings: []ValidationFinding{{
					Severity: "blocking",
					Rule:     "validation_error",
					File:     dir,
					Message:  err.Error(),
				}},
			}
		}
		res.Specs = append(res.Specs, *r)
		if r.Valid {
			res.ValidCount++
		} else {
			res.InvalidCount++
		}
	}
	res.Total = len(res.Specs)
	res.Valid = res.Total > 0 && res.InvalidCount == 0
	return res, nil
}
