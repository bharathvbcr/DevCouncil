# The agent said "done." It wasn't. So I built DevCouncil.

A few months ago I watched a coding agent finish a task, flash a green *"all tests pass,"* and hand it back to me. It looked done. It wasn't. One edge case — an empty input — quietly returned the wrong thing. The tests the agent wrote never checked for it, so the green checkmark was true and useless at the same time.

That was the moment DevCouncil started.

## The real problem isn't bad code

We keep talking about AI writing *bad* code. That's not what I kept hitting. The code was usually fine. The problem was that the agent was **confident about work it had never actually proven**. It lost track of a requirement from three messages ago. It added a dependency nobody authorized. It claimed a test passed without showing the new logic was ever run. Every failure had the same shape: the model's confidence and the actual evidence had drifted apart, and nothing in my workflow was checking the gap.

If you've vibecoded anything past a toy, you know this feeling. You prompt, you accept, you move on — and the bill for whatever the model glossed over comes due later, usually at the worst possible time.

So I stopped trying to make the agent *better* and started trying to make it *prove itself*.

## What DevCouncil actually does

DevCouncil is a command-line orchestrator that sits *beside* your coding agent — Claude Code, Codex, Cursor, Aider, whatever you use — and owns the parts agents are worst at: the plan, the scope, the verification, the repair loop, and the evidence trail.

One idea sits under all of it: **evidence, not model confidence, is the final authority.**

Concretely, it builds a persistent Requirement → Task → Diff → Evidence graph. A one-line goal gets debated into typed requirements and a scoped task list by a small "council" — two independent planners, two adversarial critics, a rebuttal round, an arbiter. Each task can then only touch the files it declared and run the commands it's allowed to. When the agent says it's finished, DevCouncil verifies the diff four ways — did it stay in scope, did the tests actually pass, was the *new* logic exercised, and is this real code instead of a stub — and it turns each acceptance criterion into a runnable check. If the evidence isn't there, the task doesn't pass. It's *blocked*, with a specific gap. Not a vibe.

That's the whole personality of the tool: it would rather tell you "I can't prove this yet" than hand you a green checkmark it didn't earn.

## It reads your codebase before it touches it

A whole class of AI coding failures happens *before* any test runs: the agent edits blind. It renames a function without knowing what else calls it, "fixes" one module, and silently breaks three that imported it. The model never had a map of the code — just the handful of files you happened to paste into the chat.

So DevCouncil builds one. `dev map` parses your repo with tree-sitter into a symbol-level graph — files, subsystems, imports, call sites — and keeps it fresh on its own (an ordinary edit marks it stale; a *missing* map is treated as stale, never trusted). It works out the important files and subsystem boundaries for *any* repo by ranking the import graph, with no configuration.

That map earns its keep twice. It grounds planning in *real* files and symbols instead of hallucinated ones. And right before a task changes a file, DevCouncil hands the agent the **blast radius** — every file that imports the one being edited — so it edits in place and keeps the call sites working instead of starting blind. You can walk the graph yourself, too: `dev graph` traces callers, flags dead code with confidence tiers, and renders the whole codebase as an interactive map.

Prevention up front, proof at the end — the same idea pointed from both directions: give the model the context to get it right, then check that it actually did.

## Does it actually work? I built a benchmark to find out

It's easy to build something like this and fall in love with the idea. I wanted to know if it moved the needle or if I'd just built an elaborate placebo.

So I wrote an adversarial benchmark. The same terse goal goes to two arms: the raw agent alone, and the agent wrapped in DevCouncil. Every task ships a **hidden** ground-truth test suite — edge cases, bad input, error handling — that the agent never sees and that only gets applied at scoring time. That's the trick. It separates "passed the happy path" from "actually correct."

The early numbers, on a GLM-5.2 planner with a Claude Sonnet executor:

- Correctness went from **0.81 (raw agent) to 0.94 (gated)** — a **+0.14 lift**.
- **Zero false negatives** — nothing broken slipped through the gate as "done."
- About **$8.49** in planning cost for the run.

But the number I actually care about isn't the lift. It's *calibration*: when the gate says "done," is it really done? And the flip side — how often did it catch a silent defect the raw agent shipped with a smile? Turning false confidence into an honest, named gap is the entire point of the project, and the benchmark is built to measure exactly that, not just headline accuracy.

## Why I haven't run the "full" benchmark yet

Here's the honest part, because it matters more than the number.

That result is **three tasks.** They're small, self-contained, single-function problems, chosen precisely because their ground truth is unambiguous and fast to score. That design favors clarity over realism — it does *not* tell you how DevCouncil behaves on a sprawling production codebase, and I won't pretend it does.

I haven't run a big SWE-bench-scale sweep yet for a few plain reasons. Coding agents are stochastic, so a credible large run means many repeats, which means real money and real wall-clock time. And a bigger suite doesn't automatically buy a *better* signal — the trustworthy read here is the relative, arm-to-arm comparison, not the absolute score, and that comparison is already visible at small N.

I'd rather ship a small, real, fully-caveated result than a big impressive one I quietly cherry-picked. That's the thesis of the project pointed back at itself: show the evidence, mark exactly how far it goes, don't oversell the confidence. The bigger runs are coming — I just won't dress up the early signal as more than it is.

## What this means if you code with AI

If you're a vibecoder, DevCouncil is a safety net, not a leash. Keep vibing on *what* you want built. Let something else hold the line on *is it actually correct.* The agent's "all tests pass" stops being something you take on faith and becomes something that got checked — and when it can't be checked, you get a specific, fixable gap instead of a surprise in production.

For teams, it's the same value in a different suit: every change is authorized, scoped, tested, and traceable back to a requirement, with a report you can review like an engineering artifact instead of a chat log you have to trust.

AI is going to write more and more of our code. I don't think the bottleneck is how much it can generate. It's whether we can trust what it hands back. DevCouncil is my bet on the answer.

**Trust the model, but verify the graph.**

It's open source (Apache 2.0) and on npm — `npm install -g devcouncil`. Kick the tires, and tell me where it breaks.
