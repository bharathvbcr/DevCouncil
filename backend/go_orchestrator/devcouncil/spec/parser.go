package spec

import (
	"bufio"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
)

var (
	reNeedsClarification = regexp.MustCompile(`\[NEEDS CLARIFICATION[^\]]*\]`)
	reUserStory          = regexp.MustCompile(`(?i)^###\s+(?:US(\d+)|User\s+Story\s+(\d+))[:\s]+(.*?)(?:\s*\((?:Priority:\s*)?(P\d+)[^)]*\))?$`)
	reSuccessMetric      = regexp.MustCompile(`(?i)\*\*(SC-\d+)\*\*[:\s]+(.*)`)
	reTaskCheckbox       = regexp.MustCompile(`^-\s+\[([ xX])\]\s+(.*)$`)
	rePhaseHeader        = regexp.MustCompile(`(?i)^##\s+Phase\s+(\d+)[:\s]*(.*)$`)
	reBacktickFile       = regexp.MustCompile("`([^`]+)`")
	reTestCommand        = regexp.MustCompile(`(?i)tests?:\s*([^\n\r]+)`)
	reDependsOn          = regexp.MustCompile(`(?i)depends_on:\s*\[([^\]]*)\]`)
	reTaskBracketID      = regexp.MustCompile(`^\[([A-Za-z0-9_-]+)\]\s*(.*)$`)
	reTaskColonID        = regexp.MustCompile(`^([A-Za-z0-9_-]+)[:\s]+(.*)$`)
	reStoryRef           = regexp.MustCompile(`^\[(US\d+|US-[A-Za-z0-9_-]+)\]\s*(.*)$`)
)

// ResolveSpecDir locates the specification directory given a feature name, ID, or direct path.
func ResolveSpecDir(projectRoot, nameOrPath string) (string, error) {
	candidates := []string{
		nameOrPath,
		filepath.Join(projectRoot, nameOrPath),
		filepath.Join(projectRoot, "specs", nameOrPath),
		filepath.Join(projectRoot, ".devcouncil", "specs", nameOrPath),
	}

	for _, cand := range candidates {
		if fi, err := os.Stat(cand); err == nil && fi.IsDir() {
			return cand, nil
		}
	}

	// Try prefix matching against entries in specs/ and .devcouncil/specs/
	for _, parent := range []string{filepath.Join(projectRoot, "specs"), filepath.Join(projectRoot, ".devcouncil", "specs")} {
		entries, err := os.ReadDir(parent)
		if err == nil {
			for _, entry := range entries {
				if entry.IsDir() {
					dirName := entry.Name()
					if dirName == nameOrPath ||
						strings.HasPrefix(dirName, nameOrPath+"-") ||
						strings.HasPrefix(dirName, nameOrPath+"_") ||
						strings.TrimLeft(strings.Split(dirName, "-")[0], "0") == strings.TrimLeft(nameOrPath, "0") {
						return filepath.Join(parent, dirName), nil
					}
				}
			}
		}
	}

	return "", fmt.Errorf("specification directory not found for: %s", nameOrPath)
}

// ParseSpecification reads and parses the 7-layer hierarchy under specDir.
func ParseSpecification(specDir string) (*Specification, error) {
	fi, err := os.Stat(specDir)
	if err != nil || !fi.IsDir() {
		return nil, fmt.Errorf("spec directory invalid: %s", specDir)
	}

	dirName := filepath.Base(specDir)
	spec := &Specification{
		FeatureID: dirName,
		SpecPath:  filepath.Join(specDir, "spec.md"),
		PlanPath:  filepath.Join(specDir, "plan.md"),
	}

	// Check auxiliary layer paths
	if path := filepath.Join(specDir, "data-model.md"); fileExists(path) {
		spec.DataModelPath = path
	}
	if path := filepath.Join(specDir, "research.md"); fileExists(path) {
		spec.ResearchPath = path
	}
	if path := filepath.Join(specDir, "contracts"); dirExists(path) {
		spec.ContractsPath = path
	}
	if path := filepath.Join(specDir, "tasks.md"); fileExists(path) {
		spec.TasksPath = path
	}
	if path := filepath.Join(specDir, "quickstart.md"); fileExists(path) {
		spec.QuickstartPath = path
	}

	// Parse spec.md
	if fileExists(spec.SpecPath) {
		if err := parseSpecFile(spec); err != nil {
			return nil, err
		}
	}

	// Parse tasks.md
	if spec.TasksPath != "" && fileExists(spec.TasksPath) {
		tasks, err := ParseTasksFile(spec.TasksPath)
		if err != nil {
			return nil, err
		}
		spec.Tasks = tasks
	}

	return spec, nil
}

func parseSpecFile(spec *Specification) error {
	f, err := os.Open(spec.SpecPath)
	if err != nil {
		return err
	}
	defer f.Close()

	scanner := bufio.NewScanner(f)
	var currentStory *UserStory
	var currentScenario *AcceptanceScenario
	inScenario := false
	lineNum := 0

	for scanner.Scan() {
		lineNum++
		line := scanner.Text()
		trimmed := strings.TrimSpace(line)

		// Scan for [NEEDS CLARIFICATION]
		if matches := reNeedsClarification.FindAllString(line, -1); len(matches) > 0 {
			for _, m := range matches {
				spec.Ambiguities = append(spec.Ambiguities, fmt.Sprintf("line %d: %s", lineNum, m))
			}
		}

		// Feature Title
		if strings.HasPrefix(trimmed, "# ") && spec.Title == "" {
			title := strings.TrimPrefix(trimmed, "# ")
			if strings.HasPrefix(strings.ToLower(title), "feature:") {
				parts := strings.SplitN(title, ":", 2)
				if len(parts) > 1 {
					title = strings.TrimSpace(parts[1])
				}
			}
			spec.Title = title
			continue
		}

		// User Story header
		if m := reUserStory.FindStringSubmatch(trimmed); len(m) > 0 {
			if currentStory != nil {
				if currentScenario != nil {
					currentStory.AcceptanceScenarios = append(currentStory.AcceptanceScenarios, *currentScenario)
					currentScenario = nil
				}
				spec.UserStories = append(spec.UserStories, *currentStory)
			}
			storyNum := m[1]
			if storyNum == "" {
				storyNum = m[2]
			}
			priority := m[4]
			if priority == "" {
				priority = "P1"
			}
			currentStory = &UserStory{
				ID:          "US" + storyNum,
				Title:       strings.TrimSpace(m[3]),
				Priority:    priority,
				Independent: true,
			}
			inScenario = false
			continue
		}

		// Acceptance Scenario
		if strings.HasPrefix(trimmed, "- **Scenario") || strings.HasPrefix(trimmed, "#### Scenario") {
			if currentStory != nil {
				if currentScenario != nil {
					currentStory.AcceptanceScenarios = append(currentStory.AcceptanceScenarios, *currentScenario)
				}
				parts := strings.SplitN(trimmed, ":", 2)
				title := "Scenario"
				if len(parts) > 1 {
					title = strings.TrimSpace(parts[1])
				}
				currentScenario = &AcceptanceScenario{Title: title}
				inScenario = true
			}
			continue
		}

		// Given / When / Then
		if inScenario && currentScenario != nil {
			if strings.HasPrefix(trimmed, "- **Given**") || strings.HasPrefix(trimmed, "**Given**") {
				currentScenario.Given = extractClause(trimmed, "Given")
			} else if strings.HasPrefix(trimmed, "- **When**") || strings.HasPrefix(trimmed, "**When**") {
				currentScenario.When = extractClause(trimmed, "When")
			} else if strings.HasPrefix(trimmed, "- **Then**") || strings.HasPrefix(trimmed, "**Then**") {
				currentScenario.Then = extractClause(trimmed, "Then")
			}
		}

		// Success Metrics
		if m := reSuccessMetric.FindStringSubmatch(trimmed); len(m) > 2 {
			spec.SuccessMetrics = append(spec.SuccessMetrics, SuccessMetric{
				ID:          m[1],
				Description: strings.TrimSpace(m[2]),
			})
		}
	}

	if currentStory != nil {
		if currentScenario != nil {
			currentStory.AcceptanceScenarios = append(currentStory.AcceptanceScenarios, *currentScenario)
		}
		spec.UserStories = append(spec.UserStories, *currentStory)
	}

	return scanner.Err()
}

// ParseTasksFile parses tasks.md content into SpecTask structs.
func ParseTasksFile(path string) ([]SpecTask, error) {
	content, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	return ParseTasksContent(string(content))
}

// ParseTasksContent parses the text of tasks.md.
func ParseTasksContent(content string) ([]SpecTask, error) {
	var tasks []SpecTask
	scanner := bufio.NewScanner(strings.NewReader(content))
	currentPhase := 1
	taskSeq := 1

	for scanner.Scan() {
		line := strings.TrimSpace(scanner.Text())

		// Phase header
		if m := rePhaseHeader.FindStringSubmatch(line); len(m) > 1 {
			if p, err := strconv.Atoi(m[1]); err == nil {
				currentPhase = p
			}
			continue
		}

		// Task checkbox item
		m := reTaskCheckbox.FindStringSubmatch(line)
		if len(m) < 3 {
			continue
		}

		completed := strings.ToLower(m[1]) == "x"
		rawRest := strings.TrimSpace(m[2])

		task := SpecTask{
			Phase:     currentPhase,
			Completed: completed,
		}

		// Check for depends_on: [...]
		if depMatch := reDependsOn.FindStringSubmatch(rawRest); len(depMatch) > 1 {
			depStr := strings.TrimSpace(depMatch[1])
			if depStr != "" {
				for _, d := range strings.Split(depStr, ",") {
					clean := strings.TrimSpace(d)
					if clean != "" {
						task.DependsOn = append(task.DependsOn, clean)
					}
				}
			}
			rawRest = strings.TrimSpace(reDependsOn.ReplaceAllString(rawRest, ""))
		}

		// Extract test commands
		if testMatch := reTestCommand.FindStringSubmatch(rawRest); len(testMatch) > 1 {
			cmd := strings.TrimSpace(testMatch[1])
			if cmd != "" {
				task.TestCommands = append(task.TestCommands, cmd)
			}
			rawRest = strings.TrimSpace(reTestCommand.ReplaceAllString(rawRest, ""))
		}

		// Extract repository-relative target files from backticks, excluding prose
		// symbols (`mcp::open_for_call`), CLI flags (`--root`), globs, and bare extensions.
		task.TargetFiles = extractTargetFilesFromBackticks(rawRest)

		// Extract Task ID
		if idMatch := reTaskBracketID.FindStringSubmatch(rawRest); len(idMatch) > 2 {
			task.ID = idMatch[1]
			rawRest = strings.TrimSpace(idMatch[2])
		} else if idMatch := reTaskColonID.FindStringSubmatch(rawRest); len(idMatch) > 2 {
			task.ID = idMatch[1]
			rawRest = strings.TrimSpace(idMatch[2])
		} else {
			task.ID = fmt.Sprintf("TASK-%03d", taskSeq)
		}
		taskSeq++

		// Check for [P] parallel marker
		if strings.HasPrefix(rawRest, "[P]") || strings.HasPrefix(rawRest, "[p]") {
			task.ParallelSafe = true
			rawRest = strings.TrimSpace(rawRest[3:])
		}

		// Check for [US#] story ref
		if storyMatch := reStoryRef.FindStringSubmatch(rawRest); len(storyMatch) > 2 {
			task.UserStoryRef = storyMatch[1]
			rawRest = strings.TrimSpace(storyMatch[2])
		}

		task.Description = rawRest
		tasks = append(tasks, task)
	}

	return tasks, scanner.Err()
}

func extractClause(line, keyword string) string {
	idx := strings.Index(line, "**"+keyword+"**")
	if idx >= 0 {
		sub := line[idx+len(keyword)+4:]
		return strings.TrimSpace(strings.TrimPrefix(sub, ":"))
	}
	return strings.TrimSpace(line)
}

func fileExists(p string) bool {
	fi, err := os.Stat(p)
	return err == nil && !fi.IsDir()
}

func dirExists(p string) bool {
	fi, err := os.Stat(p)
	return err == nil && fi.IsDir()
}

func extractTargetFilesFromBackticks(rawRest string) []string {
	matches := reBacktickFile.FindAllStringSubmatch(rawRest, -1)
	if len(matches) == 0 {
		return nil
	}
	var slashPaths []string
	var rootFiles []string
	seenSlash := make(map[string]bool)
	seenRoot := make(map[string]bool)

	for _, fm := range matches {
		if len(fm) < 2 {
			continue
		}
		cand := strings.TrimSpace(fm[1])
		if !isPlausibleRepoRelFile(cand) {
			continue
		}
		if strings.Contains(cand, "/") {
			if !seenSlash[cand] {
				seenSlash[cand] = true
				slashPaths = append(slashPaths, cand)
			}
		} else {
			if !seenRoot[cand] {
				seenRoot[cand] = true
				rootFiles = append(rootFiles, cand)
			}
		}
	}
	// When explicit directory-qualified paths are named on the task line, inline
	// bare filenames (`overlay.json`, `hook.rs`) in the prose description are prose
	// references rather than root-level files.
	if len(slashPaths) > 0 {
		return slashPaths
	}
	return rootFiles
}

func isPlausibleRepoRelFile(s string) bool {
	if s == "" || strings.HasPrefix(s, "/") || strings.HasPrefix(s, "-") || strings.HasSuffix(s, "/") {
		return false
	}
	if strings.ContainsAny(s, " \t\r\n\\*!<>()=:;$|\"'") {
		return false
	}
	parts := strings.Split(s, "/")
	for _, seg := range parts {
		if seg == "" || seg == "." || seg == ".." {
			return false
		}
	}
	base := parts[len(parts)-1]
	if strings.HasPrefix(base, ".") {
		switch base {
		case ".gitignore", ".ignore", ".gitattributes", ".editorconfig", ".dockerignore":
			return true
		default:
			return false
		}
	}
	dotIdx := strings.LastIndexByte(base, '.')
	if dotIdx <= 0 || dotIdx == len(base)-1 {
		// Allow conventional extensionless build files when directory-qualified or at root.
		switch base {
		case "Makefile", "Dockerfile", "Justfile", "justfile":
			return true
		default:
			return false
		}
	}
	ext := base[dotIdx+1:]
	for i := 0; i < len(ext); i++ {
		c := ext[i]
		if !((c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9')) {
			return false
		}
	}
	return len(ext) <= 12
}
