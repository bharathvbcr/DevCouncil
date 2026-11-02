package main

import (
	"path/filepath"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/console"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/spec"
)

const specHelp = `devcouncil spec — specification-driven workflow tools

Usage:
  devcouncil spec init <feature-name> [--project-root DIR]
  devcouncil spec validate <feature-name> [--strict] [--json] [--project-root DIR]
  devcouncil spec export-tasks <feature-name|feature-dir> [--target-status STATUS] [--dry-run] [--json] [--project-root DIR]

Commands:
  init          Scaffold a new 7-layer specification under specs/###-<feature-name>/
  validate      Statically validate a specification against schema and constitutional rules
  export-tasks  Parse tasks.md and generate DevCouncil workbench tasks and scope envelopes
`

func runSpec(args []string) int {
	if len(args) < 1 {
		console.Error(specHelp)
		return 2
	}

	switch args[0] {
	case "-h", "--help":
		console.Error(specHelp)
		return 0
	case "init":
		return runSpecInit(args[1:])
	case "validate":
		return runSpecValidate(args[1:])
	case "export-tasks":
		return runSpecExportTasks(args[1:])
	default:
		console.Errorf("unknown spec subcommand: %s\n", args[0])
		console.Error(specHelp)
		return 2
	}
}

func runSpecInit(args []string) int {
	root := ""
	featureName := ""

	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "-h", "--help":
			console.Error(specHelp)
			return 0
		case "--project-root":
			i++
			r, ok := projectRootFlag(args, i)
			if !ok {
				return 2
			}
			root = r
		default:
			if featureName == "" && args[i][0] != '-' {
				featureName = args[i]
			} else {
				console.Errorf("unexpected argument: %s\n", args[i])
				return 2
			}
		}
	}

	if featureName == "" {
		console.Errorln("spec init requires a feature name (e.g. dev spec init realtime-sync)")
		return 2
	}

	root, err := resolveRoot(root)
	if err != nil {
		console.Errorf("spec init: %v\n", err)
		return 2
	}

	dir, err := spec.ScaffoldFeature(root, featureName)
	if err != nil {
		console.Errorf("spec init failed: %v\n", err)
		return 1
	}

	rel, err := filepath.Rel(root, dir)
	if err != nil {
		rel = dir
	}
	console.Printf("Initialized 7-layer specification in %s\n", rel)
	return 0
}

func runSpecValidate(args []string) int {
	root := ""
	featureName := ""
	strict := false
	jsonOut := false

	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "-h", "--help":
			console.Error(specHelp)
			return 0
		case "--strict":
			strict = true
		case "--json":
			jsonOut = true
		case "--project-root":
			i++
			r, ok := projectRootFlag(args, i)
			if !ok {
				return 2
			}
			root = r
		default:
			if featureName == "" && args[i][0] != '-' {
				featureName = args[i]
			} else {
				console.Errorf("unexpected argument: %s\n", args[i])
				return 2
			}
		}
	}

	if featureName == "" {
		console.Errorln("spec validate requires a feature name or path")
		return 2
	}

	root, err := resolveRoot(root)
	if err != nil {
		console.Errorf("spec validate: %v\n", err)
		return 2
	}

	specDir, err := spec.ResolveSpecDir(root, featureName)
	if err != nil {
		console.Errorf("spec validate: %v\n", err)
		return 1
	}

	parsed, err := spec.ParseSpecification(specDir)
	if err != nil {
		console.Errorf("spec validate: failed to parse: %v\n", err)
		return 1
	}

	result, err := spec.ValidateSpecification(parsed, strict)
	if err != nil {
		console.Errorf("spec validate: %v\n", err)
		return 1
	}

	if jsonOut {
		if err := console.JSON(result); err != nil {
			console.Errorf("spec validate json: %v\n", err)
			return 1
		}
	} else {
		if result.Valid {
			console.Printf("Specification %s conforms to Constitution and schema rules (%d user stories, %d tasks, %d metrics)\n",
				result.FeatureID, result.StoryCount, result.TaskCount, result.MetricCount)
		} else {
			console.Printf("Specification %s failed validation with %d findings:\n", result.FeatureID, len(result.Findings))
			for _, f := range result.Findings {
				console.Printf("  [%s] %s: %s (%s)\n", f.Severity, f.Rule, f.Message, f.File)
			}
		}
	}

	if !result.Valid {
		return 1
	}
	return 0
}

func runSpecExportTasks(args []string) int {
	root := ""
	featureName := ""
	targetStatus := "ready"
	dryRun := false
	jsonOut := false

	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "-h", "--help":
			console.Error(specHelp)
			return 0
		case "--dry-run":
			dryRun = true
		case "--json":
			jsonOut = true
		case "--target-status":
			i++
			if i >= len(args) {
				console.Errorln("--target-status requires a status value")
				return 2
			}
			targetStatus = args[i]
		case "--project-root":
			i++
			r, ok := projectRootFlag(args, i)
			if !ok {
				return 2
			}
			root = r
		default:
			if featureName == "" && args[i][0] != '-' {
				featureName = args[i]
			} else {
				console.Errorf("unexpected argument: %s\n", args[i])
				return 2
			}
		}
	}

	if featureName == "" {
		console.Errorln("spec export-tasks requires a feature name or directory")
		return 2
	}

	root, err := resolveRoot(root)
	if err != nil {
		console.Errorf("spec export-tasks: %v\n", err)
		return 2
	}

	specDir, err := spec.ResolveSpecDir(root, featureName)
	if err != nil {
		console.Errorf("spec export-tasks: %v\n", err)
		return 1
	}

	res, err := spec.ExportTasks(root, specDir, targetStatus, dryRun)
	if err != nil {
		console.Errorf("spec export-tasks failed: %v\n", err)
		return 1
	}

	if jsonOut {
		if err := console.JSON(res); err != nil {
			console.Errorf("spec export-tasks json: %v\n", err)
			return 1
		}
	} else {
		mode := "exported"
		if dryRun {
			mode = "dry-run previewed"
		}
		console.Printf("Successfully %s %d tasks from %s (target status: %s)\n",
			mode, res.TotalTasks, filepath.Base(specDir), res.TargetStatus)
		for _, t := range res.Tasks {
			pMarker := ""
			if t.ParallelSafe {
				pMarker = " [P]"
			}
			console.Printf("  - %s%s: %s\n", t.TaskID, pMarker, t.Title)
		}
	}

	return 0
}
