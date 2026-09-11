---
name: data-engineering
title: Data Engineering / Pipelines / Warehouses Intake
description: Before changing data pipelines, transformations, or warehouse models, retrieve current engine/tool versions, schema and partitioning conventions, idempotency and backfill guidance, and the right run/test CLI commands — like a senior data engineer briefing themselves.
triggers:
  keywords: [etl, elt, "data pipeline", "data engineering", airflow, dagster, prefect, dbt, spark, pyspark, flink, beam, snowflake, bigquery, redshift, databricks, "data warehouse", "data lake", lakehouse, iceberg, "delta lake", parquet, partition, backfill, ingestion]
  globs: ["dbt_project.yml", "profiles.yml", "airflow.cfg", "dagster.yaml", "*.dbt", "dbt_packages", "great_expectations.yml", "*.avsc"]
---

# Data Engineering / Pipelines / Warehouses Intake

Do this **before** changing pipelines, transformations, or warehouse models. Don't rely on
training data — engines, SQL dialects, and orchestration APIs change, and a careless change
can silently corrupt data or break downstream consumers. Confirm against the engine/tool's
current docs and the project's own conventions.

## Establish current state first

1. **Engine & tool versions in use** — read the manifests (`dbt_project.yml`, `requirements`,
   cluster/runtime config): orchestrator (Airflow/Dagster/Prefect), transform tool (dbt/Spark),
   warehouse/engine (Snowflake/BigQuery/Redshift/Databricks) and its SQL dialect. Match them.
2. **Schema, contracts & lineage** — the tables/models this touches, their schema, primary/unique
   keys, and the downstream consumers (BI, models, other DAGs). A column rename or type change is
   a breaking contract — coordinate it and update the dependents.
3. **Idempotency & partitioning** — pipelines must be safe to re-run: idempotent writes
   (MERGE/upsert, partition overwrite), correct partition/cluster keys, and no duplicate or
   out-of-order side effects. Confirm how backfills are run and bounded.
4. **Data quality** — null/uniqueness/referential tests, row-count and freshness checks, and
   what happens on failure (block vs warn). Add or update tests for the change.
5. **Cost & scale** — scan/partition pruning, avoid full-table rewrites, and watch warehouse
   compute/credits for big backfills.

## Build & CLI tools

- `dbt run`/`dbt test`/`dbt build` (with `--select` to scope), `airflow dags test`/`tasks test`,
  `dagster job execute`, `spark-submit`/`pyspark`. Prefer the project's wrapper/Makefile.
- Validate transforms on a dev schema/sample before touching production datasets.

## What to record before coding

- The engine/dialect and the exact models/DAGs you will change.
- Schema/contract changes and the downstream consumers to coordinate with.
- The idempotency/partition strategy and how a backfill will be scoped and rolled back.
- The `dbt test`/DAG-test/data-quality commands that prove the change is correct.

Don't broaden the change beyond the task — no incidental model refactors or schema churn on
unrelated tables (see the surgical-changes rule in core-engineering).
