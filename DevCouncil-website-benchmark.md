# Website copy: benchmark section for devcouncil.vbcr.dev

Paste-ready copy for the marketing site. Nothing here is deployed by this
repository — `devcouncil.vbcr.dev` is hosted separately.

Every number traces to
[the committed benchmark report](benchmarks/results/competition/20260914-v0.2.2/REPORT.md).
If you re-run the benchmark, update this file and the site together, or the
site becomes a stale claim.

---

## 1. Page metadata

Target query cluster: *code intelligence for AI agents*, *code graph MCP
server*, *devmap vs gitnexus*, *codebase context for Claude Code*.

```html
<title>DevMap — Fast Code Intelligence &amp; Code Graph for AI Coding Agents</title>
<meta name="description" content="Symbol-level code graph, blast radius, and dead-code analysis for AI coding agents. Benchmarked against GitNexus, CodeGraph, Graphify, Gortex, and codebase-memory-mcp across four repositories: fastest cold index and refresh on all four, 10ms symbol queries.">
<link rel="canonical" href="https://devcouncil.vbcr.dev/">

<meta property="og:type" content="website">
<meta property="og:title" content="DevMap — Code Intelligence for AI Coding Agents">
<meta property="og:description" content="Fastest cold index and refresh on all four benchmarked repositories, 10ms symbol queries, 5/5 caller pairs. Measured, with the raw evidence published.">
<meta property="og:url" content="https://devcouncil.vbcr.dev/">
<meta name="twitter:card" content="summary_large_image">
```

Keep the description under ~155 characters if your renderer truncates; the
version above is deliberately fact-dense rather than adjective-dense.

---

## 2. Hero section

> ### Your AI agent is guessing about your codebase.
>
> DevMap gives it a symbol-level map instead — every definition, caller, and
> blast radius, in milliseconds. Six code-graph tools indexed the same four
> repositories. DevMap was fastest on cold indexing and refresh on **every one
> of them**, fastest on every symbol query we measured, and the only tool
> besides one to find all five hand-inspected caller pairs.
>
> **[See the benchmark →]** &nbsp; **[Install →]**

Alternate, more specific headline if you want the keyword up front:

> ### Code intelligence for AI coding agents — 2 seconds cold, 10 ms per query.

---

## 3. Benchmark section

> ## Measured against five other code-graph tools
>
> Not an estimate. Six tools indexed the same **four repositories** — 595 to
> 4,335 files across Rust, Go, TypeScript, Python and Swift — on one machine.
> Every corpus pinned to a commit, every competitor binary hash-verified, every
> command's raw output published.

**Fastest on all four repositories:**

| Stage | DevMap 0.2.2 | Next fastest |
|---|---|---|
| Cold index | 0.76 s – 4.61 s | 1.6× to 21× slower |
| Unchanged refresh | 0.05 s – 0.14 s | 2.2× to 107× slower |

Detailed, on the 1,098-file corpus where correctness was also checked:

| Tool | Cold index | Refresh | Edit | Symbol query | Peak RSS | Callers found |
|---|---:|---:|---:|---:|---:|---:|
| **DevMap 0.2.2** | **2.01 s** | **89 ms** | 816 ms | **9.7 ms** | **678 MiB** | **5/5** |
| CodeGraph 1.6.0 | 3.29 s | 235 ms | **512 ms** | 101–106 ms | 2430 MiB | 3/5 |
| codebase-memory-mcp 0.10.8 | 9.36 s | 5.73 s | 8.89 s | ~3.9 s | — | **5/5** |
| Graphify 0.9.59 | 14.99 s | 5.08 s | 4.88 s | 557–572 ms | 3417 MiB | 3/5 |
| GitNexus 1.6.9 | 33.69 s | 699 ms | 31.84 s | ~800 ms | 3355 MiB | 3/5 |
| Gortex 0.64.3 | 16.5 s to query-ready | — | — | 92–104 ms | — | 4/5 |

> **Where we lose.** CodeGraph re-indexes a single edited file faster than we do
> on **every** repository — 512 ms to our 816 ms here, and 3.6× faster on the
> largest gap. Graphify's index is a third the size of ours and it used less
> memory than we did on the biggest repository. All of it is in the report,
> because a comparison that only lists wins isn't a comparison.
>
> **What it doesn't prove.** Four repositories, one machine, five hand-inspected
> caller pairs. It measures speed and a small correctness sample — not general
> graph accuracy, and not whether your agent ships better code. DevMap itself
> reports 106,217 call sites it could not attribute on this corpus.
>
> **[Read the full report, including the failures →]**

That "where we lose" block is doing real work. Publishing the loss is what
makes the wins believable, and it is the same principle the product sells.

---

## 4. FAQ block

Answers questions people actually type, and gives assistants clean text to
quote. Pair with the `FAQPage` schema below.

**What is DevMap?**
A code intelligence engine that builds a symbol-level graph of a repository —
definitions, callers, blast radius, and dead code — across 35 languages via
tree-sitter. It runs as a CLI and as an MCP server for Claude Code, Cursor, and
Codex.

**How is it different from grep or embedding search?**
Grep finds text; embeddings find things that look similar. DevMap resolves
actual symbol relationships, so "who calls this function" returns callers
rather than lines that mention the name. For reference, ripgrep searched the
same corpus in ~45 ms — DevMap's resolved symbol lookup was 9.7 ms.

**Is it faster than GitNexus / CodeGraph / Graphify?**
On the four benchmarked repositories: faster than all five on cold indexing and
unchanged refresh on every one, and faster on every symbol query measured.
CodeGraph is faster at re-indexing a single edited file, on every repository.

**Which languages are supported?**
35 named tree-sitter extractors plus a generic fallback, including TypeScript,
Python, Go, Rust, Java, C#, C/C++, Swift, Kotlin, Ruby, PHP, Scala, Dart,
Solidity, and Terraform.

**Is it free?** Apache-2.0, and the benchmark is reproducible from the repo.

---

## 5. Structured data (JSON-LD)

Drop into `<head>`. Schema helps search engines and assistants parse what this
is. Do not add `aggregateRating` or `review` unless you have genuine
user-submitted ratings — fabricated review markup is a manual-action risk.

```html
<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@type": "SoftwareApplication",
  "name": "DevCouncil",
  "alternateName": "DevMap",
  "applicationCategory": "DeveloperApplication",
  "operatingSystem": "macOS, Linux, Windows",
  "description": "Code intelligence and deterministic verification for AI coding agents: symbol-level code graph, blast radius, dead-code analysis, and MCP servers for Claude Code, Cursor, and Codex.",
  "url": "https://devcouncil.vbcr.dev/",
  "codeRepository": "https://github.com/bharathvbcr/DevCouncil",
  "programmingLanguage": ["Rust", "Go"],
  "license": "https://www.apache.org/licenses/LICENSE-2.0",
  "softwareVersion": "0.2.2",
  "offers": { "@type": "Offer", "price": "0", "priceCurrency": "USD" }
}
</script>
```

```html
<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@type": "FAQPage",
  "mainEntity": [
    {
      "@type": "Question",
      "name": "What is DevMap?",
      "acceptedAnswer": {
        "@type": "Answer",
        "text": "DevMap is a code intelligence engine that builds a symbol-level graph of a repository — definitions, callers, blast radius, and dead code — across 35 languages via tree-sitter. It runs as a CLI and as an MCP server for Claude Code, Cursor, and Codex."
      }
    },
    {
      "@type": "Question",
      "name": "Is DevMap faster than GitNexus or CodeGraph?",
      "acceptedAnswer": {
        "@type": "Answer",
        "text": "Across four benchmarked repositories, DevMap was fastest at cold indexing and unchanged refresh on all four. On the 1,098-file corpus it indexed cold in 2.01 seconds against CodeGraph's 3.29 and GitNexus's 33.69, and answered symbol queries in 9.7 ms against 101-106 ms and ~800 ms respectively. CodeGraph was faster at re-indexing a single edited file on every repository, at 512 ms against DevMap's 816 ms on this corpus."
      }
    },
    {
      "@type": "Question",
      "name": "Which languages does DevMap support?",
      "acceptedAnswer": {
        "@type": "Answer",
        "text": "35 named tree-sitter language extractors plus a generic fallback, including TypeScript, Python, Go, Rust, Java, C#, C/C++, Swift, Kotlin, Ruby, PHP, Scala, Dart, Solidity, and Terraform."
      }
    }
  ]
}
</script>
```

---

## 6. Technical SEO checklist for the site

Not verified from this repo — check each against the live deployment:

- [ ] `homepageUrl` on GitHub points at the site — **done**, adds a backlink.
- [ ] Site links back to the repo with a plain `<a href>`, not a JS-only handler.
- [ ] One `<h1>` per page, containing the primary keyword.
- [ ] `sitemap.xml` and `robots.txt` served, sitemap submitted in Search Console.
- [ ] Interactive graph showcase has real server-rendered text around it — a
      canvas or WebGL demo is invisible to crawlers on its own.
- [ ] Core Web Vitals: the graph visualiser is the likely LCP/INP risk.
- [ ] `og:image` — a screenshot of the graph view is the strongest share asset.
- [ ] Cross-link site ↔ GitHub ↔ npm ↔ crates.io so all four reinforce.
