package main

import (
	"fmt"
	"os"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/console"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/dc/store"
	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/devcouncil/gatescfg"
)

const doctorHelp = `devcouncil doctor — which store next_task reads, and what it holds

Usage:
  devcouncil doctor [--json] [--project-root DIR]

Reports the task store (the workbench.db config key, default
.devcouncil/state.sqlite), whether it can be opened, the gate mode, and how many
ready items it holds: all of them, and how many next_task would offer under the
gate mode in force (a task with a blocking gap is not offered under enforce).
Exits 1 when the store cannot be read.
`

// doctorReport is what `devcouncil doctor` says.
type doctorReport struct {
	OK          bool   `json:"ok"`
	Root        string `json:"root"`
	WorkbenchDB string `json:"workbench_db"`
	DBOrigin    string `json:"workbench_db_origin"`
	ConfigPath  string `json:"config_path"`
	GateMode    string `json:"gate_mode"`
	// ReadyItems is every ready, unheld item of this repository. Offered is how
	// many of them next_task hands out under GateMode; Blocked is the ones with
	// a blocking gap, which only advisory and off offer, and last.
	ReadyItems int      `json:"ready_items"`
	Offered    int      `json:"offered_by_next_task"`
	Blocked    int      `json:"blocked_by_gaps"`
	NextTask   string   `json:"next_task,omitempty"`
	Problems   []string `json:"problems,omitempty"`
}

func runDoctor(args []string) int {
	jsonOut := false
	root := ""
	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "-h", "--help":
			console.Error(doctorHelp)
			return 0
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
			console.Errorf("unknown flag: %s\n", args[i])
			return 2
		}
	}
	root, err := resolveRoot(root)
	if err != nil {
		console.Errorf("doctor: %v\n", err)
		return 2
	}
	report := inspectStore(root)
	if jsonOut {
		if err := console.JSON(report); err != nil {
			return 1
		}
	} else {
		printDoctor(report)
	}
	if !report.OK {
		return 1
	}
	return 0
}

// inspectStore asks the store the questions next_task depends on. Every
// question that cannot be answered is a problem, never a zero.
func inspectStore(root string) doctorReport {
	snap := gatescfg.Resolve(root)
	report := doctorReport{
		Root: root, WorkbenchDB: gatescfg.DefaultWorkbenchDB(root), DBOrigin: gatescfg.OriginDefault,
		ConfigPath: snap.ConfigPath, GateMode: snap.VerificationMode,
	}
	if snap.Error != "" {
		report.Problems = append(report.Problems, "gate config: "+snap.Error)
	}
	loc, err := gatescfg.ResolveWorkbenchDB(root)
	report.WorkbenchDB, report.DBOrigin = loc.Path, loc.Origin
	if err != nil {
		report.Problems = append(report.Problems, "workbench.db: "+err.Error())
		return report
	}
	if _, err := os.Stat(loc.Path); err != nil {
		report.Problems = append(report.Problems, fmt.Sprintf("no task store at %s (create a task with `devcouncil task create`)", loc.Path))
		return report
	}
	client := dcstoreClient(root, loc.Path)
	if client == nil {
		report.Problems = append(report.Problems, "no dcstore can run against the task store (`devcouncil install dcstore`)")
		return report
	}
	defer client.Close()
	ctx := console.Context()
	if err := client.Available(ctx); err != nil {
		report.Problems = append(report.Problems, err.Error())
		return report
	}
	// All of it, then what the gate mode lets next_task offer: two asks, so the
	// difference is visible rather than assumed.
	all, err := client.ReadyTasks(ctx, store.ReadyAdvisory)
	if err != nil {
		report.Problems = append(report.Problems, "ready: "+err.Error())
		return report
	}
	offered, err := client.ReadyTasks(ctx, store.ReadyMode(report.GateMode))
	if err != nil {
		report.Problems = append(report.Problems, "ready: "+err.Error())
		return report
	}
	report.ReadyItems, report.Offered = len(all), len(offered)
	for _, t := range all {
		if !t.ReadyToCheckout {
			report.Blocked++
		}
	}
	if len(offered) > 0 {
		report.NextTask = offered[0].ID
	}
	report.OK = len(report.Problems) == 0
	return report
}

func printDoctor(r doctorReport) {
	console.Printf("task store:   %s (%s)\n", r.WorkbenchDB, r.DBOrigin)
	console.Printf("gate mode:    %s\n", r.GateMode)
	if r.ReadyItems > 0 || r.OK {
		console.Printf("ready items:  %d (%d offered by next_task under %s, %d blocked by gaps)\n",
			r.ReadyItems, r.Offered, r.GateMode, r.Blocked)
		if r.NextTask != "" {
			console.Printf("next task:    %s\n", r.NextTask)
		}
	}
	for _, p := range r.Problems {
		console.Printf("problem:      %s\n", p)
	}
}
