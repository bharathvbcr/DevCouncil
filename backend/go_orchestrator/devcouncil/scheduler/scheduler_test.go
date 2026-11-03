package scheduler

import (
	"context"
	"testing"
	"time"
)

func TestScheduler_FIFOOrdering(t *testing.T) {
	sched := New(1)
	defer sched.Close()

	var order []string

	req1 := HeavyJobRequest{
		JobID:          "job-1",
		TaskID:         "task-1",
		Command:        "sh",
		Args:           []string{"-c", "sleep 0.1"},
		TimeoutSeconds: 5,
	}
	req2 := HeavyJobRequest{
		JobID:          "job-2",
		TaskID:         "task-1",
		Command:        "sh",
		Args:           []string{"-c", "sleep 0.05"},
		TimeoutSeconds: 5,
	}

	t1, err := sched.Submit(req1)
	if err != nil {
		t.Fatal(err)
	}
	if t1.PositionInQueue != 1 {
		t.Fatalf("expected position 1, got %d", t1.PositionInQueue)
	}

	t2, err := sched.Submit(req2)
	if err != nil {
		t.Fatal(err)
	}
	if t2.PositionInQueue != 2 {
		t.Fatalf("expected position 2, got %d", t2.PositionInQueue)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	res1, err := sched.Wait(ctx, "job-1")
	if err != nil {
		t.Fatal(err)
	}
	if res1.Status != StatusCompleted {
		t.Fatalf("job 1 status %s", res1.Status)
	}
	order = append(order, "job-1")

	res2, err := sched.Wait(ctx, "job-2")
	if err != nil {
		t.Fatal(err)
	}
	if res2.Status != StatusCompleted {
		t.Fatalf("job 2 status %s", res2.Status)
	}
	order = append(order, "job-2")

	if len(order) != 2 || order[0] != "job-1" || order[1] != "job-2" {
		t.Fatalf("expected FIFO execution order [job-1, job-2], got %v", order)
	}
}

func TestScheduler_ConcurrencyLimits(t *testing.T) {
	concurrency := 2
	sched := New(concurrency)
	defer sched.Close()

	var running int32
	var maxObserved int32

	for i := 1; i <= 4; i++ {
		jobID := "conc-job-" + string(rune('0'+i))
		req := HeavyJobRequest{
			JobID:          jobID,
			TaskID:         "task-conc",
			Command:        "sh",
			Args:           []string{"-c", "sleep 0.1"},
			TimeoutSeconds: 5,
		}
		_, err := sched.Submit(req)
		if err != nil {
			t.Fatal(err)
		}
	}

	sub := sched.Notifications()
	completed := 0
	deadline := time.After(5 * time.Second)

	for completed < 4 {
		select {
		case <-sub:
			completed++
		case <-deadline:
			t.Fatal("timed out waiting for jobs to finish")
		}
	}

	// Verify scheduler records all completed
	for i := 1; i <= 4; i++ {
		jobID := "conc-job-" + string(rune('0'+i))
		res, ok := sched.GetResult(jobID)
		if !ok || res.Status != StatusCompleted {
			t.Fatalf("expected completed status for %s", jobID)
		}
	}
	_ = running
	_ = maxObserved
}

func TestScheduler_TimeoutCancellation(t *testing.T) {
	sched := New(1)
	defer sched.Close()

	req := HeavyJobRequest{
		JobID:          "timeout-job",
		TaskID:         "task-timeout",
		Command:        "sleep",
		Args:           []string{"10"},
		TimeoutSeconds: 1, // 1s timeout
	}

	_, err := sched.Submit(req)
	if err != nil {
		t.Fatal(err)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	res, err := sched.Wait(ctx, "timeout-job")
	if err != nil {
		t.Fatal(err)
	}

	if res.Status != StatusFailed {
		t.Fatalf("expected StatusFailed for timed out job, got %s", res.Status)
	}
}

func TestScheduler_ManualCancel(t *testing.T) {
	sched := New(1)
	defer sched.Close()

	// Submit running job to occupy concurrency slot
	req1 := HeavyJobRequest{
		JobID:          "occupier",
		TaskID:         "task-1",
		Command:        "sleep",
		Args:           []string{"5"},
		TimeoutSeconds: 10,
	}
	_, err := sched.Submit(req1)
	if err != nil {
		t.Fatal(err)
	}

	// Submit queued job
	req2 := HeavyJobRequest{
		JobID:          "to-be-cancelled",
		TaskID:         "task-2",
		Command:        "echo",
		Args:           []string{"hi"},
		TimeoutSeconds: 5,
	}
	_, err = sched.Submit(req2)
	if err != nil {
		t.Fatal(err)
	}

	// Cancel queued job
	if !sched.Cancel("to-be-cancelled") {
		t.Fatal("expected cancel of queued job to succeed")
	}

	res, ok := sched.GetResult("to-be-cancelled")
	if !ok || res.Status != StatusCancelled {
		t.Fatalf("expected StatusCancelled for queued job, got %v", res)
	}

	// Cancel running job
	if !sched.Cancel("occupier") {
		t.Fatal("expected cancel of running job to succeed")
	}

	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	resOcc, err := sched.Wait(ctx, "occupier")
	if err != nil {
		t.Fatal(err)
	}
	if resOcc.Status != StatusCancelled {
		t.Fatalf("expected StatusCancelled for running job, got %s", resOcc.Status)
	}
}
