# Environment variables

rigg is configured by `rigg.yaml` and by flags. Environment variables cover
the two things a file cannot: what the *session* is (which environment,
whether anyone is at the keyboard) and who rigg is *authenticating as*. A
handful more exist to point rigg at a fake Azure during its own tests.

Every variable listed here is read by rigg's own code. Flags always win over
variables, and `rigg.yaml` always wins over both for anything it can express.

## Contents

- [Session](#session)
- [Authentication](#authentication)
- [Tuning](#tuning)
- [Test-only](#test-only)
- [Not environment variables](#not-environment-variables)
- [Recipes](#recipes)
- [Common mistakes](#common-mistakes)

## Session

| Variable | Read by | Default | Meaning |
|---|---|---|---|
| `RIGG_ENV` | every command that targets an environment | — | Selects the environment, as if `--env` had been passed |
| `RIGG_NON_INTERACTIVE` | every command | unset | Forces non-interactive behaviour while still at a terminal |
| `RIGG_NO_UPDATE_CHECK` | `rigg` startup | unset | Skips the background release check |

**`RIGG_ENV`** — order of precedence: `--env` > `RIGG_ENV` > the environment
marked `default: true`.

**`RIGG_NON_INTERACTIVE`** — any value counts as "on" **except** the empty
string, `0`, and `false` (case-insensitive), so `RIGG_NON_INTERACTIVE=` from
a CI template does not silently disable prompting.

**`RIGG_NO_UPDATE_CHECK`** — any value works, even an empty one. Already
implied by `--quiet` and by `rigg mcp serve`.

rigg also treats a session as non-interactive when `--non-interactive`,
`--yes` or `--output json` is given, or when stdin or stdout is not a
terminal. In a non-interactive session a guided flow does not prompt: it
prints a `needs-input` document and exits 6. See
[Exit codes and questions](exit-codes-and-questions.md#needs-input).

## Authentication

rigg resolves an access token per `(tenant, audience)` pair, highest
precedence first:

1. `RIGG_ACCESS_TOKEN`
2. the service-principal variables below
3. the operator's Azure CLI sign-in (`az login`)

| Variable | Read by | Default | Meaning |
|---|---|---|---|
| `RIGG_ACCESS_TOKEN` | the token chain | — | A pre-minted bearer token, honoured for **every** audience |
| `AZURE_CLIENT_ID` | the token chain | — | Service-principal (or workload-identity) client id |
| `AZURE_TENANT_ID` | the token chain | — | The service principal's home tenant |
| `AZURE_CLIENT_SECRET` | the token chain | — | Client secret |
| `AZURE_FEDERATED_TOKEN_FILE` | the token chain | — | Path to an OIDC assertion from a workload-identity issuer |

**`RIGG_ACCESS_TOKEN`** wins over everything. It is intended for tests and
for hosts that mint their own tokens; it is not a general-purpose credential,
because one token cannot really be valid for Search, Foundry, ARM, Key Vault
and Graph at once.

**`AZURE_TENANT_ID`** — an environment's own `tenant:` in `rigg.yaml` wins
over it when both are present.

**`AZURE_CLIENT_SECRET`** is sent in the token-request form body; it is never
logged and never placed in a process argument.

**`AZURE_FEDERATED_TOKEN_FILE`** is written by a workload-identity issuer
(GitHub Actions, AKS). It **wins over `AZURE_CLIENT_SECRET`** when both are
set, and is re-read at every token request because the issuer rotates it.
This is the credential `rigg ci init`'s workflows use — no secret is stored
anywhere.

> [!WARNING]
> rigg takes the service-principal path only when **all three** of
> `AZURE_CLIENT_ID`, `AZURE_TENANT_ID` and one of `AZURE_CLIENT_SECRET` /
> `AZURE_FEDERATED_TOKEN_FILE` are set and non-empty. With anything less it
> falls back to the Azure CLI, silently — which is the right behaviour on a
> workstation and the wrong surprise in CI.

`AZURE_SUBSCRIPTION_ID` is *not* read by rigg — the subscription comes from
`rigg.yaml`. It appears in the workflows `rigg ci init` writes because the
`azure/login` action wants it, and `rigg ci init` prints a reminder to set it
as a repository variable alongside `AZURE_CLIENT_ID` and `AZURE_TENANT_ID`.

## Tuning

| Variable | Read by | Default | Meaning |
|---|---|---|---|
| `RIGG_RBAC_RETRY_SECS` | `rigg push` | `10` | Seconds between attempts while waiting for a role assignment |
| `RIGG_RBAC_MAX_RETRIES` | `rigg push` | `18` | How many such attempts to make |
| `RIGG_WATCH_INTERVAL_SECS` | `rigg az indexer run --watch` | `5` | Seconds between indexer status polls |

**The two RBAC variables** cover both waiting for a role assignment rigg has
created to become visible and retrying a write that failed on a
not-yet-propagated grant. Raise both in a tenant where RBAC propagation is
slow; a grant that never lands is a warning, not a refusal.

## Test-only

These exist so rigg's own test suite can point a client at a local fake
instead of Azure. They also happen to be the escape hatch for a sovereign
cloud, but the supported way to move an endpoint is `endpoint:` in
`rigg.yaml`, which is per environment rather than per process.

| Variable | Replaces | Default |
|---|---|---|
| `RIGG_ARM_ENDPOINT` | the Azure Resource Manager base URL | `https://management.azure.com` |
| `RIGG_LOGIN_ENDPOINT` | the Entra ID token endpoint host | `https://login.microsoftonline.com` |
| `RIGG_GRAPH_ENDPOINT` | the Microsoft Graph base URL (route-versioned) | `https://graph.microsoft.com/v1.0` |
| `RIGG_KEYVAULT_ENDPOINT` | the vault host — it replaces the vault URI entirely | the binding's own vault URI |
| `RIGG_OPENAPI_DIR` | — | unset |

**`RIGG_OPENAPI_DIR`** points at a local checkout of `azure-rest-api-specs`
for an ignored test that re-derives rigg's pinned schema fixtures. It is not
used at runtime.

Each endpoint variable is ignored when set to the empty string, and a
trailing `/` is trimmed.

> [!NOTE]
> The Search and Foundry data planes have **no** endpoint variable — their
> override is `endpoint:` under `search:` / `foundry:` in `rigg.yaml`, which
> is what rigg's own sync tests use together with `RIGG_ACCESS_TOKEN`.

## Not environment variables

The scanner that keeps this page honest also matches Rust constant names, so
for completeness: `X_RIGG_API`, `X_RIGG_AUTH`, `X_RIGG_AUTH_FUNCTION_KEY`,
`X_RIGG_AUTH_KEY_VAULT_PREFIX`, `X_RIGG_PIN` and `X_RIGG_REF` are registry
constants naming the `x-rigg-*` JSON annotations, not variables rigg reads
from the environment. They are documented in
[Annotations](annotations.md).

## Recipes

**CI with a federated credential** — what `rigg ci init` produces. No secret
exists to leak:

```bash
export AZURE_CLIENT_ID=00000000-0000-0000-0000-000000000000
export AZURE_TENANT_ID=11111111-1111-1111-1111-111111111111
export AZURE_FEDERATED_TOKEN_FILE=/var/run/secrets/azure/tokens/azure-identity-token
export RIGG_ENV=prod
export RIGG_NON_INTERACTIVE=1
rigg validate
rigg diff --all --exit-code
```

**CI with a client secret** — when federation is not available. Keep the
secret in the CI system's secret store, never in a file:

```bash
export AZURE_CLIENT_ID=00000000-0000-0000-0000-000000000000
export AZURE_TENANT_ID=11111111-1111-1111-1111-111111111111
export AZURE_CLIENT_SECRET=***
rigg push --all --confirm-env prod
```

**Scripted behaviour at a terminal** — verify how a flow behaves in CI
without leaving your shell:

```bash
RIGG_NON_INTERACTIVE=1 rigg promote contoso-docs --from dev --to prod --output json
```

## Common mistakes

**`RIGG_ENV` left over in the shell.** It outranks `default: true`, so a
forgotten export silently retargets every command. `rigg status` and every
other command name the environment they acted on — read that line.

**`RIGG_NON_INTERACTIVE=false` expecting to force prompting.** `false`, `0`
and the empty string all mean "off"; the variable only ever *disables*
prompting. To force prompting, unset it.

**Setting only two of the three service-principal variables.** rigg does not
fail — it quietly falls back to the Azure CLI, so on a workstation everything
keeps working under *your* identity and in CI you get a sign-in error
(`Not logged in to Azure CLI. Run: az login`) that looks nothing like the
missing variable that caused it. `rigg auth status` reports
`Environment Variables: Configured` only when the full set is present — check
there first.

When a credential file is named but unreadable, the error is explicit:

```text
Authentication failed: could not read the federated token file AZURE_FEDERATED_TOKEN_FILE=/var/run/secrets/azure/tokens/azure-identity-token: No such file or directory (os error 2)
```

**Using `RIGG_ACCESS_TOKEN` for real work.** It is honoured for every
audience, so a token minted for Search will be sent to ARM, Key Vault and
Graph as well — and rejected there in ways that look like permission bugs.
Use the service-principal variables or `az login`.

**Setting `AZURE_SUBSCRIPTION_ID` and expecting rigg to use it.** It does
not; put `subscription:` in the environment block in `rigg.yaml`.

## See also

- [rigg.yaml](rigg-yaml.md) — `tenant`, `subscription`, `endpoint` and the environment model.
- [Exit codes and questions](exit-codes-and-questions.md) — what non-interactive mode does when a flow needs an answer.
- [`CONCEPTS.md`](../../CONCEPTS.md) — how rigg handles authentication, the principals and the requirement graph.
- [CLI reference](cli.md#rigg-ci) — `rigg ci init`, `rigg auth status`, `rigg auth login`.
