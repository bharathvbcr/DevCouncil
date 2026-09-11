---
name: backend
title: Backend / API / Services Intake
description: Before writing server-side code, retrieve current framework and runtime versions, security and auth guidance, database/migration tooling, and the right CLI/build/test commands — like a senior backend engineer briefing themselves on the stack.
triggers:
  keywords: [backend, server, api, rest, grpc, graphql, microservice, fastapi, django, flask, starlette, express, nestjs, rails, spring, laravel, gin, fiber, echo, axum, actix, sqlalchemy, alembic, prisma, postgres, postgresql, mysql, sqlite, redis, mongodb, celery, rabbitmq, kafka, docker, kubernetes, migration, webhook, jwt, oauth]
  globs: ["manage.py", "wsgi.py", "asgi.py", "alembic.ini", "Dockerfile", "docker-compose.yml", "docker-compose.yaml", "go.mod", "Cargo.toml", "*.proto", "Gemfile", "Procfile", "main.go", "openapi.yaml", "openapi.json"]
---

# Backend / API / Services Intake

Do this **before** writing or changing server-side code. Don't rely on training data —
frameworks, runtimes, and security guidance change, and a pattern that was fine a year
ago may now be deprecated or insecure. Confirm against the framework's current docs and
release notes, and against the project's own dependency manifests.

## Establish current state first

1. **Runtime & framework versions in use** — read the dependency manifest
   (`pyproject.toml`/`requirements.txt`, `package.json`, `go.mod`, `Cargo.toml`, `Gemfile`,
   `pom.xml`) and any lockfile: language runtime version, web framework (FastAPI/Django/
   Flask/Express/NestJS/Gin/Axum/Spring/Rails), and ORM/data layer. Match what's already there.
2. **Current stable versions & deprecations** — the framework's latest stable release and
   anything deprecated/removed between the project's version and current (routing, lifecycle
   hooks, settings, async APIs). List the ones this change touches and their replacements.
3. **Data & migrations** — how schema changes are managed (Alembic, Django migrations,
   Prisma Migrate, golang-migrate). Never hand-edit schema without a migration; generate one
   and confirm it runs forward and back.
4. **Security & correctness** — input validation, authn/authz on every new endpoint, secrets
   from config/env (never hard-coded), parameterized queries (no string-built SQL), safe
   defaults for CORS/headers, and rate limits where relevant. Check the framework's current
   security guidance for the version in use.
5. **Contracts** — if there's an API schema (OpenAPI, `.proto`, GraphQL SDL), update it with
   the change and keep it the source of truth; note backward-compatibility for existing clients.

## Build & CLI tools

- Run/serve and migrate via the project's tooling: `uvicorn`/`gunicorn`, `python manage.py`,
  `npm run`/`pnpm`, `go run`/`go build`, `cargo run`/`cargo build`, `rails`, `./gradlew`.
- Tests: `pytest`, `go test ./...`, `cargo test`, `npm test`, `python manage.py test` — prefer
  the command the repo already uses so the change is verifiable.
- Containers: `docker build` / `docker compose up` when the service is containerized.

## What to record before coding

- The runtime/framework versions and the exact dependencies you will use.
- Deprecated APIs to avoid and their modern replacements, with migration steps.
- Any new migration to generate, and the auth/validation each new endpoint enforces.
- The build/test commands you will run so the change is provably correct.

Don't broaden the change beyond the task — no incidental framework upgrades or schema
churn on unrelated tables (see the surgical-changes rule in core-engineering).
