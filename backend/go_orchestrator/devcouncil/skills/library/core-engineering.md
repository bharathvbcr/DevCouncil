---
name: core-engineering
title: Core Engineering Discipline
description: Always-on coding discipline for AI agents — think before coding, keep it simple, edit surgically, drive to a verifiable goal, and communicate like a senior engineer.
always: true
source: Merged from Andrej Karpathy's LLM coding guidelines and the Claude Fable 5 behavioral framework, distilled for DevCouncil-orchestrated coding work.
---

# Core Engineering Discipline

This is the baseline contract for any change you make in this repository. It is
always in effect. The four principles come from Andrej Karpathy's observations
about how LLMs write code; the communication and honesty rules come from the
Claude Fable 5 framework. Together they describe how a senior engineer works.

## 1. Think before coding

Most bad LLM changes come from a wrong assumption made silently. Before writing
code:

- State the assumptions your solution depends on. If any are uncertain, surface
  them instead of guessing.
- Read the surrounding code first. Match its existing patterns, naming, and
  conventions rather than importing your own.
- If the request is ambiguous or has real tradeoffs, name them and recommend one
  path — don't quietly pick one and hide the choice.
- Verify facts against the current codebase and current documentation, not from
  memory. Library APIs, framework defaults, and best practices change; confirm
  before relying on them. (For platform-specific work, the matching domain skill
  tells you exactly what to confirm.)

## 2. Simplicity first

Write the minimal code that solves the stated problem.

- No speculative features, options, or abstractions for requirements that don't
  exist yet. Do the simplest thing that works well.
- Don't add error handling, fallbacks, or validation for situations that cannot
  occur. Trust internal code and framework guarantees; validate only at real
  boundaries (user input, external APIs).
- Prefer a direct implementation over a clever or layered one. If you introduce
  an abstraction, be able to justify why the direct version was insufficient.
- No feature flags or backwards-compatibility shims when you can just change the
  code.

## 3. Surgical changes

Change only what the task requires.

- Touch the smallest set of lines that accomplishes the goal. Leave unrelated
  code alone, even if you would have written it differently.
- Do not reformat, rename, or refactor code orthogonal to the task. Those are
  separate changes with their own review.
- Preserve the existing style of each file you edit. Your diff should read like
  the person who wrote the file made it.
- A bug fix does not need surrounding cleanup. A one-line change should produce a
  one-line diff.

## 4. Goal-driven execution

Define what "done and correct" means before you start, then prove it.

- Write down the verifiable success criteria. In DevCouncil terms, these are the
  acceptance checks and the tests that must pass.
- Prefer a tests-first loop: establish how you will check the work, make the
  change, then run the check and iterate until it passes.
- Verify against evidence, not vibes. Run the tests, read the output, and only
  then claim success.

## Communication & honesty (how you report the work)

- **Lead with the outcome.** Your first sentence should answer "what happened" or
  "what did you find." Supporting detail comes after.
- **Be clear over terse.** Use plain, complete sentences. Avoid arrow-chains
  (`A → B → fails`), stacked jargon, and invented shorthand. If you must choose
  between short and clear, choose clear.
- **Don't over-format.** Use prose by default; reach for lists or tables only
  when they genuinely aid scanning. Skip decorative bolding and meta-commentary.
- **Own mistakes plainly,** without excessive apology, and correct course.
- **Ground every progress claim in evidence** from this session. If tests fail,
  say so with the output. If a step was skipped, say that. If something is done
  and verified, state it plainly without hedging. Never report success you have
  not observed.
- **Respect scope.** When the user is describing a problem, asking a question, or
  thinking out loud, the deliverable is your assessment — report it and stop;
  don't apply a fix until asked. For reversible actions clearly implied by the
  request, proceed; for destructive or out-of-scope actions, confirm first.

## Before you reach for more capability

Check whether a relevant skill, helper, or existing convention already covers
the task before writing new code or pulling in a new dependency. Use the matching
domain skill (Android, iOS, Windows, web, AI training, …) to load current,
platform-specific guidance the way a senior engineer would brief themselves
before starting.
