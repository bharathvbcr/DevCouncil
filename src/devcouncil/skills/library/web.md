---
name: web
title: Web / Frontend Development Intake
description: Before writing web code, retrieve current framework versions, runtime/build tooling, deprecations, and recommended patterns — like a senior web engineer briefing themselves on the stack.
triggers:
  keywords: [web, website, frontend, react, next, nextjs, vue, svelte, angular, typescript, javascript, vite, tailwind, node]
  globs: ["package.json", "tsconfig.json", "*.tsx", "*.jsx", "*.vue", "*.svelte", "next.config.*", "vite.config.*", "tailwind.config.*"]
---

# Web / Frontend Development Intake

Do this **before** writing or changing web code. The JS/TS ecosystem moves quickly
and major versions change defaults and APIs. Confirm against the framework's official
docs and the project's `package.json` — not from memory.

## Establish current state first

1. **Framework & versions** — read `package.json` (and lockfile): the framework
   (React/Next, Vue/Nuxt, Svelte/SvelteKit, Angular), its major version, the build
   tool (Vite, Next, Webpack), the package manager (npm/pnpm/yarn/bun), and the Node
   version (`engines`, `.nvmrc`). Match what's already in use.
2. **Latest stable & major-version shifts** — current stable major and any defaults
   that changed (e.g. React Server Components / the App Router, Vue 3 Composition API,
   Svelte 5 runes, ESM-only packages). Note what gates this task.
3. **Deprecations** — APIs/patterns deprecated in the project's major version (e.g.
   legacy lifecycle methods, `getInitialProps`, options API where composition is
   preferred). List the ones this change touches and their replacements.
4. **Recommended patterns** — TypeScript strictness, data-fetching/caching model,
   state management, styling approach (CSS modules, Tailwind, CSS-in-JS), and
   accessibility (semantic HTML, ARIA only where needed, keyboard support).
5. **Guidelines** — performance budgets (Core Web Vitals), accessibility (WCAG), and
   SSR/CSR/SSG choice relevant to the change.

## Build & CLI tools

- Package manager scripts (`npm run build`/`test`/`lint`, or pnpm/yarn/bun equivalents).
- The framework CLI (`next`, `vite`, `ng`, `svelte-kit`) for dev/build.
- `eslint`/`prettier`/`tsc --noEmit` and the test runner (Vitest/Jest/Playwright) if configured.

## What to record before coding

- Framework + major version, build tool, package manager, and Node version.
- Deprecated patterns to avoid and their modern replacements.
- The build/test/lint commands you will run so the change is verifiable.

Don't introduce a second styling system or state library when one is already in use,
and don't bump a major framework version as a side effect of an unrelated task.
