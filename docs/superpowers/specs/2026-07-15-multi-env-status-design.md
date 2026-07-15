# Multi-environment `rigg status`

**Status:** Approved design
**Date:** 2026-07-15

## Problem

`rigg status` reports exactly one environment (flag > `RIGG_ENV` > `default: true`),
while the workspace is inherently multi-env: files live under `envs/<env>/` for every
environment, and `promote`/`diff --compare-env` are cross-env verbs. Questions like
"is prod in sync while dev is ahead?" require one invocation per environment.

## Behavior

- `rigg status [<project>]` reports **every** environment in `rigg.yaml` by default,
  with full per-resource drift detail per environment.
- Explicit selection narrows to one env, with today's precedence: `--env` flag >
  `RIGG_ENV`. `RIGG_ENV` continues to narrow (it is an explicit selection; a CI job
  with `RIGG_ENV=prod` must not fan out to dev).
- The `default: true` environment prints first; the rest in alphabetical order.
- Per-env degradation: an unreachable env (auth failure, network) renders a single
  clear error line for that env; other envs render fully.

## Output

Text — env becomes the outer grouping, per-project sections unchanged inside:

```
env: dev (default)
  rag
    indexes/products                 in sync
env: prod
  auth failed (...) — run `rigg auth doctor`
```

JSON — new top-level shape (breaking, accepted):

```json
[
  {"env": "dev", "default": true, "error": null, "projects": [ ...today's objects... ]},
  {"env": "prod", "default": false, "error": "auth failed: ...", "projects": []}
]
```

MCP `rigg_status` description changes from "scoped to ONE environment" to "all
environments unless `env` is set". No schema change (the `env` param already exists).

## Internals

- Extract the per-env body of `commands/status.rs` into `async fn env_report(...)`
  returning pure data: per-project rows + unmanaged lists, or a failure
  `{ reason, is_auth }`. Errors are caught inside each env's future.
- Fan out over environments with `join_all`; render text/JSON after all complete.
- Token cache in `rigg-client` `auth.rs`: process-wide `scope → (token, acquired_at)`
  map, reused within 5 minutes, so N envs × M requests do not each spawn an `az`
  subprocess. This also speeds up single-env commands, which today shell out per
  request.

## Exit codes

0 normally; 4 only if **all** environments fail auth. A partially degraded run exits 0
— status observes, it does not gate (drift has never changed status's exit code).

## Testing

- `crates/rigg/tests/sync.rs` (wiremock): two-env workspace pointed at two wiremock
  servers — both env sections present in text and JSON; one env returning 401 →
  degraded line + exit 0; all envs failing auth → exit 4.
- `crates/rigg/tests/cli_surface.rs`: envs without connections → LocalOnly rows per env.
- Token cache unit test in `auth.rs`.

## Out of scope

`rigg env list`, `promote`, `diff --compare-env` unchanged.
