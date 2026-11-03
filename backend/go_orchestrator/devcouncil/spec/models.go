package spec

// Specification represents the complete parsed 7-layer hierarchy for a feature.
type Specification struct {
	FeatureID      string            `json:"feature_id"`
	Title          string            `json:"title"`
	SpecPath       string            `json:"spec_path"`
	PlanPath       string            `json:"plan_path"`
	DataModelPath  string            `json:"data_model_path,omitempty"`
	ResearchPath   string            `json:"research_path,omitempty"`
	ContractsPath  string            `json:"contracts_path,omitempty"`
	TasksPath      string            `json:"tasks_path,omitempty"`
	QuickstartPath string            `json:"quickstart_path,omitempty"`
	Constitution   ConstitutionCheck `json:"constitution"`
	UserStories    []UserStory       `json:"user_stories"`
	SuccessMetrics []SuccessMetric   `json:"success_metrics"`
	Tasks          []SpecTask        `json:"tasks"`
	Ambiguities    []string          `json:"ambiguities"`
}

// ConstitutionCheck records validation against constitution.md articles.
type ConstitutionCheck struct {
	Passed       bool     `json:"passed"`
	Violations   []string `json:"violations"`
	ArticlesRead []string `json:"articles_read"`
}

// UserStory maps to a prioritized slice of functionality.
type UserStory struct {
	ID                  string               `json:"id"`       // e.g. "US1"
	Priority            string               `json:"priority"` // "P1", "P2", "P3"
	Title               string               `json:"title"`
	Description         string               `json:"description,omitempty"`
	AcceptanceScenarios []AcceptanceScenario `json:"acceptance_scenarios"`
	Independent         bool                 `json:"independent"`
}

// AcceptanceScenario represents a Gherkin Given/When/Then specification.
type AcceptanceScenario struct {
	Title string `json:"title"`
	Given string `json:"given"`
	When  string `json:"when"`
	Then  string `json:"then"`
}

// SuccessMetric defines a measurable verification criterion.
type SuccessMetric struct {
	ID          string `json:"id"` // e.g. "SC-001"
	Description string `json:"description"`
	Target      string `json:"target"`
}

// SpecTask represents a single executable unit of work in tasks.md.
type SpecTask struct {
	ID           string   `json:"id"` // e.g. "T-001", "TASK_01"
	Phase        int      `json:"phase"`
	UserStoryRef string   `json:"user_story_ref,omitempty"`
	Description  string   `json:"description"`
	ParallelSafe bool     `json:"parallel_safe"`
	Completed    bool     `json:"completed"`
	TargetFiles  []string `json:"target_files"`
	TestCommands []string `json:"test_commands"`
	DependsOn    []string `json:"depends_on"`
}

// ValidationFinding represents an issue discovered during specification validation.
type ValidationFinding struct {
	Severity string `json:"severity"` // "blocking", "advisory"
	Rule     string `json:"rule"`     // e.g. "unresolved_ambiguity", "missing_scenario"
	File     string `json:"file"`
	Line     int    `json:"line,omitempty"`
	Message  string `json:"message"`
}

// ValidationResult summarizes the outcome of validating a feature specification.
type ValidationResult struct {
	Valid       bool                `json:"valid"`
	FeatureID   string              `json:"feature_id"`
	SpecPath    string              `json:"spec_path"`
	Findings    []ValidationFinding `json:"findings"`
	StoryCount  int                 `json:"story_count"`
	TaskCount   int                 `json:"task_count"`
	MetricCount int                 `json:"metric_count"`
}
