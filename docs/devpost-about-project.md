## Inspiration

AI coding agents can produce impressive changes quickly, but they can also lose requirements, modify files outside their intended scope, or declare success without proving that the relevant behavior was tested. As projects and agent conversations grow, important decisions become scattered across prompts, terminal output, and transient chat history.

I built DevCouncil around a simple principle:

> **Model confidence should not be the final authority. Evidence should be.**

For DevCouncil, an accepted change should satisfy a concrete engineering contract:

$$
\text{Accepted change} = \text{scoped diff} + \text{passing verification} + \text{acceptance evidence}
$$

DevCouncil existed before OpenAI Build Week. This submission covers only the meaningful extensions developed on or after July 13, 2026. The pre-event baseline is commit `3cfd5d1`, and eligible history begins at `6f5bd73`.

## What it does

DevCouncil is an evidence-first control plane for AI-assisted software development. It works alongside Codex and other coding agents and maintains a persistent:

`Requirement → Task → Diff → Evidence`

workflow.

DevCouncil turns a development goal into requirements, acceptance criteria, assumptions, and scoped tasks. Each task can declare its permitted files, expected tests, allowed commands, dependencies, and lease owner. After an agent works on the task, DevCouncil inspects the real Git diff, checks whether the change stayed within scope, runs deterministic evidence commands, and links the resulting evidence back to the acceptance criteria.

If proof is missing, DevCouncil does not return a vague failure. It produces typed, machine-readable repair actions that identify whether the next step is to fix code, add a test, repair verification, resolve scope, or address a security concern. The same workflow is available through the CLI, Model Context Protocol tools, coding-agent integrations, CI, and evidence reports.

DevCouncil also builds a queryable repository map and code graph. Agents can use it to understand subsystem boundaries, callers, dependencies, dead-code candidates, execution paths, and change impact before editing unfamiliar code.

## How we built it

DevCouncil is primarily written in Python and uses SQLite as the canonical store for workflow and code-intelligence data. Typer provides the CLI, Pydantic defines durable contracts, SQLAlchemy handles persistence, and tree-sitter powers multi-language parsing. MCP exposes task, status, diff, evidence, policy, and verification tools to Codex and other compatible agents. A Node.js/npm wrapper distributes the `dev` and `devcouncil` commands and provisions the Python runtime through `uv`.

During Build Week, I meaningfully extended the project with:

- a canonical SQLite-backed code-intelligence index, multi-language grammars, incremental synchronization, graph queries, community detection, and a self-contained interactive graph artifact;
- stronger deterministic verification through stop gates, claim checking, diff-to-evidence coverage, task leases, program-dependence and corpus checks, bounded repair, and structured next actions;
- deeper Codex, MCP, CLI, dashboard, and GitHub Actions integration;
- lossless handling of project-sized structured MCP output and stricter task-scoped diff behavior;
- a provider-free red-to-green demonstration that judges can run without an API key; and
- an installable npm release path that tests the packed artifact rather than relying only on the source checkout.

I used Codex and GPT-5.6 as engineering partners to map unfamiliar execution paths, challenge correctness claims, run adversarial install and browser checks, diagnose failures from evidence, implement focused repairs, and verify the resulting behavior. I remained responsible for the product requirements, architecture, scope decisions, review, and acceptance criteria. This submission does not claim that every eligible line was authored exclusively by Codex or GPT-5.6.

## Challenges we ran into

One of the hardest problems was making incremental repository analysis agree with the authoritative full-build contract. Watchers, concurrent graph writers, generated assets, ambiguous call edges, and multi-language parsers created failure modes that happy-path tests did not reveal.

Another challenge was preserving useful information without overwhelming the agent. Real projects can produce large task descriptions, diffs, gap lists, and live-review signals. During the submission audit, valid CLI JSON was being truncated before MCP parsed it, causing core tools to report `cli_parse_error`. The fix required separating lossless internal transport from deliberately bounded external text responses.

The graph demo exposed a similar gap between unit-level confidence and real user behavior. The generated HTML referenced a ForceGraph chain method that was not available in the bundled production library. String-based tests passed, but the exact standalone page a judge would open rendered a blank canvas. Testing the packaged artifact in a browser found the problem.

Cross-platform behavior was also demanding. Filesystem events, path normalization, SQLite lifecycle behavior, debugger adapters, shell commands, and packaging behave differently across macOS, Linux, and Windows. The project needed bounded retries, explicit degraded states, and platform-focused CI rather than assuming one successful local run proved portability.

## Accomplishments that we're proud of

I am proud that DevCouncil does not ask users to trust another AI-generated confidence score. Its core gates are deterministic and inspect the actual repository, commands, tests, coverage evidence, and persisted task scope.

I am also proud of the project’s dogfooding loop. DevCouncil has been used to audit and harden its own repository mapping, watcher behavior, verification rules, MCP contracts, package installation, and demonstration artifacts. That process found real bugs—including the blank graph and large-JSON MCP failure—that would have been easy to miss in a feature-only review.

The Build Week version gives judges a provider-free path to see the central idea directly: a controlled change first fails verification, receives a real repair, and then passes with compiled evidence and zero gaps. The project is also installable through npm and exposes the same evidence model to humans, CI, and coding agents.

## What we learned

I learned that stronger models do not remove the need for engineering controls. As agents become more capable, explicit scope, durable state, recoverable execution, and evidence-linked acceptance become more important—not less.

I also learned that verification must test the user-visible contract. A unit test that finds expected strings in generated HTML does not prove that the browser can execute it. A passing CLI command does not prove that its MCP adapter preserves the complete structured response. A green test suite does not prove that changed lines were exercised. The most valuable audits followed the entire path a user or agent would take.

Finally, I learned that a technically deep system needs a deliberately small demonstration. The clearest explanation of DevCouncil is not its feature count; it is one red-to-green cycle showing that an agent’s claim becomes acceptable only when the evidence supports it.

## What's next for DevCouncil

Next, I plan to strengthen task-scoped Git diff handling for untracked, renamed, binary, and path-filtered files; reduce agent context through compact and paginated MCP responses; and remove sensitive local signal details from general status output.

On the product side, I want to turn the dashboard into a clearer executive evidence view, add packaged browser automation for the graph, improve initial graph orientation and fit-to-view behavior, and provide a supported isolated `dev demo` workflow for teams evaluating the product.

Longer term, DevCouncil will continue improving cross-process watcher verification, graph precision, language and LSP coverage, cross-platform debugger reliability, release-health reporting, and CI evidence integration. The goal remains the same: make AI-assisted development faster without lowering the standard of proof required to accept a change.
