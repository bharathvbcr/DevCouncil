package verify

// gateConfigGap is the blocking gap for a gate config that could not be read.
// The run is enforcing because of it, so the gap names the path and the error
// the operator has to fix, and a run cannot pass while it stands.
func gateConfigGap(taskID string, in Input) Gap {
	path := in.GateConfigPath
	if path == "" {
		path = ".devcouncil/config.yaml"
	}
	return Gap{
		ID:       StableGapID(taskID, "GATE-CONFIG-INVALID"),
		Severity: "high",
		GapType:  GateConfigInvalid,
		TaskID:   taskID,
		Description: "The gate config " + path + " could not be read as a gate setting, so this run " +
			"enforces instead of using the mode it names: " + in.GateConfigError,
		Evidence:       []string{path, in.GateConfigError},
		RecommendedFix: "Fix " + path + " (gates.mode is off, advisory or enforce), or run `devcouncil gate set --mode <mode>`, then re-verify.",
		Blocking:       true,
		File:           filePtr(path),
	}
}

// envConfigGap is the blocking gap for a verify.env_allow that could not be
// read. The commands ran on the base allowlist alone, so a failure may be the
// missing variable rather than the code.
func envConfigGap(taskID, reason string) Gap {
	return Gap{
		ID:       StableGapID(taskID, "VERIFY-ENV-CONFIG-INVALID"),
		Severity: "high",
		GapType:  VerifyEnvConfigInvalid,
		TaskID:   taskID,
		Description: "verify.env_allow could not be read, so the verification commands ran with the base " +
			"environment allowlist only: " + reason,
		Evidence:       []string{reason},
		RecommendedFix: "Write verify.env_allow in .devcouncil/config.yaml as an inline list, [NAME, NAME], then re-verify.",
		Blocking:       true,
	}
}
