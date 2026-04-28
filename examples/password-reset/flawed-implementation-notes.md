# Flawed Implementation Notes

This directory contains a demo scenario where an AI agent builds a password reset feature that is intentionally flawed.

## Flaws
1. Token expires, but it is reusable (not single-use).
2. Raw token is stored in the database instead of a hashed version.
3. No test evidence proves that reuse fails.

DevCouncil's Verifier should catch these gaps and produce repair tasks.
