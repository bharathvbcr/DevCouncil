---
name: devops
title: DevOps / Infrastructure-as-Code / CI-CD Intake
description: Before changing infrastructure, pipelines, or deployment config, retrieve current provider/tool versions, security and least-privilege guidance, state/secret handling, and the right plan/apply CLI commands — like a senior platform engineer briefing themselves.
triggers:
  keywords: [devops, terraform, opentofu, kubernetes, helm, kustomize, ansible, pulumi, cloudformation, bicep, gitops, argocd, infrastructure, "infrastructure as code", iac, provisioning, deployment, "ci/cd", cicd, jenkins, "github actions", iam, "least privilege"]
  globs: ["*.tf", "*.tfvars", "Chart.yaml", "kustomization.yaml", "kustomization.yml", "Jenkinsfile", "*.bicep", "playbook.yml", "ansible.cfg", "skaffold.yaml", "argocd*.yaml", "*.hcl"]
---

# DevOps / Infrastructure-as-Code / CI-CD Intake

Do this **before** changing infrastructure, pipelines, or deployment config. Don't rely
on training data — providers, modules, and runner images change fast, and an insecure or
deprecated default can be costly. Confirm against the provider/tool's current docs and the
project's own pinned versions and state.

## Establish current state first

1. **Tool & provider versions in use** — read the version pins (`required_version`,
   `required_providers`, `Chart.yaml`/`appVersion`, action `uses: @vX`, runner image,
   k8s API versions) and any lockfile (`.terraform.lock.hcl`). Match what's already there.
2. **State & backends** — where state lives (remote backend, workspace, locking). Never run
   a destructive `apply`/`destroy` blindly; read the plan first and understand what changes.
3. **Deprecations & breaking changes** — between the pinned versions and current (provider
   resource renames, removed k8s APIs, deprecated action inputs, runner image changes). List
   the ones this change touches and their replacements.
4. **Security & least privilege** — IAM/roles scoped to the minimum needed, no wildcard
   permissions, secrets from a vault/secret store (never committed), network/ingress locked
   down, and encrypted state/storage. Check the provider's current security guidance.
5. **Blast radius** — what this change can take down (shared modules, prod vs staging
   workspace, a pipeline that gates deploys). Prefer a dry run / plan and a staged rollout.

## Build & CLI tools

- `terraform`/`tofu plan|apply`, `kubectl`/`helm`/`kustomize`, `ansible-playbook --check`,
  `pulumi preview`, `aws`/`gcloud`/`az` CLIs — use the project's wrapper/Makefile if present.
- Validate before applying: `terraform validate`/`fmt`, `helm lint`/`template`, `kubeval`,
  `actionlint`, and a CI dry run.

## What to record before coding

- The pinned versions and provider/module sources you will use.
- Deprecated resources/APIs to avoid and their replacements, with migration steps.
- The plan/preview output and the blast radius, plus how you'll roll back.
- The validate/plan commands that prove the change is safe before apply.

Don't broaden the change beyond the task — no incidental provider upgrades or touching
unrelated modules/environments (see the surgical-changes rule in core-engineering).
