package scheduler

import (
	"context"
	"errors"
	"fmt"
	"os/exec"
	"runtime"
	"strings"
	"sync"
	"time"

	"github.com/bharathvbcr/DevCouncil/backend/go_orchestrator/proc"
)

type HeavyJobType string

const (
	JobCargoBuild           HeavyJobType = "cargo_build"
	JobCargoTest            HeavyJobType = "cargo_test"
	JobGoTest               HeavyJobType = "go_test"
	JobBenchmark            HeavyJobType = "benchmark"
	JobDerivedArtifactRegen HeavyJobType = "derived_artifact_regen"
)

type JobQueueStatus string

const (
	StatusQueued    JobQueueStatus = "queued"
	StatusRunning   JobQueueStatus = "running"
	StatusCompleted JobQueueStatus = "completed"
	StatusFailed    JobQueueStatus = "failed"
	StatusCancelled JobQueueStatus = "cancelled"
	StatusShedded   JobQueueStatus = "shedded"
)

// HeavyJobRequest represents an asynchronous heavy operation request.
type HeavyJobRequest struct {
	JobID          string       `json:"job_id"`
	TaskID         string       `json:"task_id"`
	AgentID        string       `json:"agent_id"`
	WorktreePath   string       `json:"worktree_path"`
	JobType        HeavyJobType `json:"job_type"`
	Command        string       `json:"command"`
	Args           []string     `json:"args"`
	TimeoutSeconds uint32       `json:"timeout_seconds"`
	MaxRSSMB       uint32       `json:"max_rss_mb"`
}

// HeavyJobTicket is returned immediately upon enqueuing.
type HeavyJobTicket struct {
	JobID                string         `json:"job_id"`
	PositionInQueue      int            `json:"position_in_queue"`
	EstimatedWaitSeconds uint32         `json:"estimated_wait_seconds"`
	Status               JobQueueStatus `json:"status"`
}

// JobResult captures the execution outcome.
type JobResult struct {
	JobID       string         `json:"job_id"`
	Status      JobQueueStatus `json:"status"`
	ExitCode    int            `json:"exit_code"`
	Output      string         `json:"output"`
	Error       string         `json:"error,omitempty"`
	StartedAt   time.Time      `json:"started_at"`
	CompletedAt time.Time      `json:"completed_at"`
}

type jobState struct {
	req      *HeavyJobRequest
	ticket   HeavyJobTicket
	result   *JobResult
	cancelFn context.CancelFunc
	doneChan chan struct{}
}

// Scheduler coordinates heavy operations using a FIFO queue and concurrency tokens.
type Scheduler struct {
	mu            sync.Mutex
	concurrency   int
	semaphore     chan struct{}
	queue         []*HeavyJobRequest
	jobs          map[string]*jobState
	notifications chan JobResult
	closed        bool
	ctx           context.Context
	cancel        context.CancelFunc
}

// DefaultConcurrency computes max(1, floor(CPU / 2)).
func DefaultConcurrency() int {
	c := runtime.NumCPU() / 2
	if c < 1 {
		return 1
	}
	return c
}

// New creates a new load-aware FIFO job scheduler.
func New(concurrency int) *Scheduler {
	if concurrency <= 0 {
		concurrency = DefaultConcurrency()
	}
	ctx, cancel := context.WithCancel(context.Background())
	s := &Scheduler{
		concurrency:   concurrency,
		semaphore:     make(chan struct{}, concurrency),
		queue:         make([]*HeavyJobRequest, 0),
		jobs:          make(map[string]*jobState),
		notifications: make(chan JobResult, 64),
		ctx:           ctx,
		cancel:        cancel,
	}
	return s
}

// Submit validates and enqueues a heavy job request, returning a ticket immediately.
func (s *Scheduler) Submit(req HeavyJobRequest) (*HeavyJobTicket, error) {
	if strings.TrimSpace(req.JobID) == "" {
		return nil, errors.New("job_id must not be empty")
	}
	if strings.TrimSpace(req.Command) == "" {
		return nil, errors.New("command must not be empty")
	}
	if req.TimeoutSeconds == 0 {
		req.TimeoutSeconds = 600 // Default 10 minute timeout
	}
	if req.MaxRSSMB == 0 {
		req.MaxRSSMB = 512
	}

	s.mu.Lock()
	defer s.mu.Unlock()

	if s.closed {
		return nil, errors.New("scheduler is closed")
	}
	if _, exists := s.jobs[req.JobID]; exists {
		return nil, fmt.Errorf("job %s already exists", req.JobID)
	}

	pos := len(s.queue) + 1
	ticket := HeavyJobTicket{
		JobID:                req.JobID,
		PositionInQueue:      pos,
		EstimatedWaitSeconds: uint32(pos * 30),
		Status:               StatusQueued,
	}

	reqCopy := req
	state := &jobState{
		req:      &reqCopy,
		ticket:   ticket,
		doneChan: make(chan struct{}),
	}
	s.jobs[req.JobID] = state
	s.queue = append(s.queue, &reqCopy)

	go s.scheduleNext()

	return &ticket, nil
}

func (s *Scheduler) scheduleNext() {
	s.mu.Lock()
	if s.closed || len(s.queue) == 0 {
		s.mu.Unlock()
		return
	}
	s.mu.Unlock()

	select {
	case s.semaphore <- struct{}{}:
		// Acquired concurrency token
	case <-s.ctx.Done():
		return
	}

	s.mu.Lock()
	if s.closed || len(s.queue) == 0 {
		s.mu.Unlock()
		<-s.semaphore
		return
	}

	// Dequeue in FIFO order
	jobReq := s.queue[0]
	s.queue = s.queue[1:]
	for i, qItem := range s.queue {
		if st, ok := s.jobs[qItem.JobID]; ok {
			st.ticket.PositionInQueue = i + 1
			st.ticket.EstimatedWaitSeconds = uint32((i + 1) * 30)
		}
	}

	state := s.jobs[jobReq.JobID]
	if state.ticket.Status == StatusCancelled {
		s.mu.Unlock()
		<-s.semaphore
		go s.scheduleNext()
		return
	}

	state.ticket.Status = StatusRunning
	state.ticket.PositionInQueue = 0
	state.ticket.EstimatedWaitSeconds = 0

	jobCtx, cancelFn := context.WithTimeout(s.ctx, time.Duration(jobReq.TimeoutSeconds)*time.Second)
	state.cancelFn = cancelFn
	s.mu.Unlock()

	go func(req *HeavyJobRequest, js *jobState, ctx context.Context, cancel context.CancelFunc) {
		defer cancel()
		defer func() {
			<-s.semaphore
			go s.scheduleNext()
		}()

		started := time.Now()
		cmd := exec.CommandContext(ctx, req.Command, req.Args...)
		if req.WorktreePath != "" {
			cmd.Dir = req.WorktreePath
		}

		proc.EnforceSubprocessInvariants(cmd)
		outBuf := &proc.CappedBuffer{Limit: 2 << 20} // 2MB output cap
		cmd.Stdout = outBuf
		cmd.Stderr = outBuf

		runErr, timedOut := proc.RunBounded(ctx, cmd.Run)
		completed := time.Now()

		res := JobResult{
			JobID:       req.JobID,
			StartedAt:   started,
			CompletedAt: completed,
			Output:      outBuf.String(),
		}

		s.mu.Lock()
		defer s.mu.Unlock()

		if js.ticket.Status == StatusCancelled {
			res.Status = StatusCancelled
			res.ExitCode = 1
			res.Error = "job was cancelled"
		} else if timedOut {
			res.Status = StatusFailed
			res.ExitCode = 124
			res.Error = fmt.Sprintf("job timed out after %d seconds", req.TimeoutSeconds)
			js.ticket.Status = StatusFailed
		} else if runErr != nil {
			res.Status = StatusFailed
			res.Error = runErr.Error()
			if exitErr, ok := runErr.(*exec.ExitError); ok {
				res.ExitCode = exitErr.ExitCode()
			} else {
				res.ExitCode = 1
			}
			js.ticket.Status = StatusFailed
		} else {
			res.Status = StatusCompleted
			res.ExitCode = 0
			js.ticket.Status = StatusCompleted
		}

		js.result = &res
		close(js.doneChan)

		select {
		case s.notifications <- res:
		default:
		}
	}(jobReq, state, jobCtx, cancelFn)
}

// Cancel attempts to cancel an in-flight or queued job.
func (s *Scheduler) Cancel(jobID string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()

	state, exists := s.jobs[jobID]
	if !exists {
		return false
	}

	if state.ticket.Status == StatusQueued {
		state.ticket.Status = StatusCancelled
		// Remove from queue
		for i, req := range s.queue {
			if req.JobID == jobID {
				s.queue = append(s.queue[:i], s.queue[i+1:]...)
				break
			}
		}
		res := JobResult{
			JobID:       jobID,
			Status:      StatusCancelled,
			ExitCode:    1,
			Error:       "job was cancelled while queued",
			CompletedAt: time.Now(),
		}
		state.result = &res
		close(state.doneChan)
		return true
	}

	if state.ticket.Status == StatusRunning {
		state.ticket.Status = StatusCancelled
		if state.cancelFn != nil {
			state.cancelFn()
		}
		return true
	}

	return false
}

// GetTicket retrieves the current ticket for a job.
func (s *Scheduler) GetTicket(jobID string) (HeavyJobTicket, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()

	state, ok := s.jobs[jobID]
	if !ok {
		return HeavyJobTicket{}, false
	}
	return state.ticket, true
}

// GetResult retrieves the finished result for a job.
func (s *Scheduler) GetResult(jobID string) (*JobResult, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()

	state, ok := s.jobs[jobID]
	if !ok || state.result == nil {
		return nil, false
	}
	resCopy := *state.result
	return &resCopy, true
}

// Wait blocks until the job finishes or ctx expires.
func (s *Scheduler) Wait(ctx context.Context, jobID string) (*JobResult, error) {
	s.mu.Lock()
	state, ok := s.jobs[jobID]
	if !ok {
		s.mu.Unlock()
		return nil, fmt.Errorf("job %s not found", jobID)
	}
	done := state.doneChan
	s.mu.Unlock()

	select {
	case <-done:
		res, _ := s.GetResult(jobID)
		return res, nil
	case <-ctx.Done():
		return nil, ctx.Err()
	}
}

// Notifications returns a read-only channel of job completion events.
func (s *Scheduler) Notifications() <-chan JobResult {
	return s.notifications
}

// Close stops the scheduler and cancels in-flight jobs.
func (s *Scheduler) Close() {
	s.mu.Lock()
	if s.closed {
		s.mu.Unlock()
		return
	}
	s.closed = true
	s.cancel()
	s.mu.Unlock()
}
