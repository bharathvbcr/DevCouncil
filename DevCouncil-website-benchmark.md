# Website copy: benchmark section for devcouncil.vbcr.dev

Paste-ready copy for the marketing site. Nothing here is deployed by this
repository — `devcouncil.vbcr.dev` is hosted separately.

Every number traces to
[the committed benchmark report](benchmarks/results/competition/20260913-48cd3c7/REPORT.md).
If you re-run the benchmark, update this file and the site together, or the
site becomes a stale claim.

---

## 1. Page metadata

Target query cluster: *code intelligence for AI agents*, *code graph MCP
server*, *devmap vs gitnexus*, *codebase context for Claude Code*.

```html
<title>DevMap — Fast Code Intelligence &amp; Code Graph for AI Coding Agents</title>
<meta name="description" content="Symbol-level code graph, blast radius, and dead-code analysis for AI coding agents. Benchmarked against GitNexus, CodeGraph, Graphify, Gortex, and codebase-memory-mcp: 2.0s cold index, 30ms symbol queries, 610MiB peak memory.">
<link rel="canonical" href="https://devcouncil.vbcr.dev/">

<meta property="og:type" content="website">
<meta property="og:title" content="DevMap — Code Intelligence for AI Coding Agents">
<meta property="og:description" content="2.0s cold index, 30ms symbol queries, lowest memory of six code-graph tools. Measured, with the raw evidence published.">
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
> blast radius, in milliseconds. Six code-graph tools indexed the same
> repository. DevMap was fastest on cold indexing, refresh, and every symbol
> query we measured, using a quarter of the memory.
>
> **[See the benchmark →]** &nbsp; **[Install →]**

Alternate, more specific headline if you want the keyword up front:

> ### Code intelligence for AI coding agents — 2 seconds cold, 30 ms per query.

---

## 3. Benchmark section

> ## Measured against five other code-graph tools
>
> Not an estimate. Six tools indexed the identical frozen 1,186-file repository
> on one machine — 261 timed samples, corpus hash-verified before and after,
> every command's raw output published.

| Tool | Cold index | Refresh | Edit | Symbol query | Peak RSS | Callers found |
|---|---:|---:|---:|---:|---:|---:|
| **DevMap 0.2.1** | **2.03 s** | **107 ms** | 958 ms | **30–33 ms** | **610 MiB** | **5/5** |
| CodeGraph 1.6.0 | 2.91 s | 242 ms | **417 ms** | 144–166 ms | 2438 MiB | 3/5 |
| codebase-memory-mcp 0.10.8 | 8.02 s | 6.16 s | 9.35 s | ~4.4 s | 1208 MiB | **5/5** |
| Graphify 0.9.59 | 16.46 s | 4.33 s | 3.95 s | 567–615 ms | 3293 MiB | 3/5 |
| GitNexus 1.6.9 | 34.10 s | 568 ms | 33.42 s | ~1.0 s | 3095 MiB | 3/5 |
| Gortex 0.64.3 | 38.84 s | 968 ms | 5.75 s | 92–115 ms | 2206 MiB | 4/5 |

> **Where we lose.** CodeGraph re-indexes a single edited file in 417 ms to our
> 958 ms, and Graphify's index is a third the size of ours. Both are in the
> report, because a comparison that only lists wins isn't a comparison.
>
> **What it doesn't prove.** One repository, one machine, five hand-inspected
> caller pairs. It measures speed and a small correctness sample — not general
> graph accuracy, and not whether your agent ships better code.
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
same corpus in ~36 ms — DevMap's resolved symbol lookup was 30–33 ms.

**Is it faster than GitNexus / CodeGraph / Graphify?**
On the benchmarked corpus: faster than all five on cold indexing, refresh, and
symbol queries. CodeGraph is faster at re-indexing a single edited file.

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
  "softwareVersion": "0.2.1",
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
        "text": "On a benchmarked 1,186-file repository, DevMap indexed cold in 2.03 seconds against CodeGraph's 2.91 and GitNexus's 34.10, and answered symbol queries in 30-33 ms. CodeGraph was faster at re-indexing a single edited file, at 417 ms against DevMap's 958 ms."
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
