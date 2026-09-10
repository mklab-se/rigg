# rigg 2.0 — Documentation (workstream 5b) design

**Status:** approved for planning (Kristofer's requirement: "documentation
updated before we release, such that a user of Rigg can understand all the
details of various configuration files, but also easy to follow tutorials of
simple use cases, explaining how Rigg works").
**Parent:** `2026-09-09-rigg-2.0-scope-and-principles-design.md` §6 row 5b.
**Written against:** the code-complete state of workstreams 0–4 (branch
`rigg-2`, after the identity workstream's final review).

## 1. Goal

A reader who has never used rigg can, from the docs alone:

1. Understand every file rigg reads or writes and every field in it.
2. Follow four tutorials end to end against their own Azure subscription and
   arrive at a working, version-controlled Agentic RAG solution.
3. Find any command, flag, environment variable, question id or exit code
   in one place, and trust that the entry is exact.

Non-goals: marketing copy, a full Azure AI Search / Foundry primer (link to
Microsoft Learn), and re-documenting internals beyond what a contributor
needs to update an API version (that stays in `CLAUDE.md` and the
api-watchdog skill).

## 2. Information architecture

Everything user-facing moves under `docs/`, the root files become short
entry points, and every reference page is either generated from the binary
or guarded by a test so it cannot drift silently.

```
README.md                      # what/why, 60-second quick start, links (≤ 200 lines)
CONCEPTS.md                    # unchanged home of `rigg concepts` (crate copy guarded)
GETTING_STARTED.md             # becomes a 20-line pointer to docs/tutorials/
MCP.md                         # stays; tool reference is regenerated (see §4)
docs/
  README.md                    # the docs index: reading order, "which page answers what"
  reference/
    rigg-yaml.md               # workspace file: every key, type, default, example
    project-yaml.md            # project manifest + the directory-is-membership rule
    resource-files.md          # per kind: path, physical `name`, sidecars, what push strips
    annotations.md             # x-rigg-api / -auth / -pin / -ref / -note: syntax + semantics
    apis.md                    # apis/<name>.json OpenAPI contract for WebApiSkill
    state.md                   # .rigg/: state.json baselines, bindings.json, answers; gitignore
    cli.md                     # GENERATED command/flag reference (§4)
    environment-variables.md   # every RIGG_* / AZURE_* variable rigg reads
    exit-codes-and-questions.md# exit codes; needs-input protocol; every question id prefix
  tutorials/
    01-pull-an-existing-solution.md
    02-build-from-scratch.md
    03-add-an-environment-and-promote.md
    04-push-to-protected-production.md
  how-rigg-works.md            # narrative: sync engine, bindings, auth graph, promote translation
```

`CONCEPTS.md` keeps its current scope (the mental model) and is linked from
`docs/README.md`; the reference pages link back to its sections rather than
repeating them.

## 3. Reference pages — required content

Each reference page follows the same shape: a one-paragraph purpose, a
complete annotated example, then a table with **key · type · required ·
default · meaning · since** for every field, then "common mistakes" with the
exact validate/exit output the user will see.

### 3.1 `rigg-yaml.md`

Every field of `WorkspaceConfig`, `Environment`, `Policy`,
`SearchConnection`, `FoundryConnection` and `Binding` in
`crates/rigg-core/src/workspace.rs`, i.e. `name`, `root`, `environments.*.
{default, tenant, subscription, search.{name, service, endpoint,
api-version, preview-api-version}, foundry.{name, account, endpoint,
project, api-version}, policy.{protected, strict-bindings}, dependencies.
<name>.{storage | ai-services | function-app | identity | key-vault | api}}`.
Includes: environment resolution order (flag > `RIGG_ENV` > `default:
true`), the implicit `search` and `foundry` bindings, multi-subscription and
multi-tenant examples, the `endpoint:` override used by tests, and what
`rigg env add|bind|unbind|show` write.

### 3.2 `project-yaml.md`

`ProjectManifest` (`description`), directory-is-membership, exclusive
ownership, the `envs/<env>/` tree, and `rigg new project`.

### 3.3 `resource-files.md`

For all 12 kinds: directory, file naming (logical id = path stem, physical
`name` inside), the fields rigg strips on pull (volatile, read-only) and on
push (`x-rigg-*`), secret rejection, `$file` sidecars, and the
infrastructure-bearing fields the binding layer rewrites (the `InfraRef`
table in `registry.rs`, rendered as a table by kind with the binding type it
maps to). The page is partly generated: `rigg dev infra-table` prints the
InfraRef rows as Markdown (§4).

### 3.4 `annotations.md`

`x-rigg-api: <spec>`, `x-rigg-auth: function-key | key-vault:<secret>@<binding>`,
`x-rigg-pin`, `x-rigg-ref: knowledge-bases/<kb>`, `x-rigg-note` — syntax,
where each is valid, how validate checks it, how promote treats it, how push
resolves it.

### 3.5 `apis.md`

The OpenAPI 3.1 contract rigg validates for WebApiSkill, `rigg new api`,
`rigg describe`'s "APIs to implement", Easy Auth expectations on the
function side.

### 3.6 `state.md`

`.rigg/<env>/<project>/state.json` (baselines, checksums, self-healing),
`.rigg/<env>/bindings.json` (learned bindings, `rigg env learn`), answered
promote bindings, what is safe to delete, and the recommended `.gitignore`.

### 3.7 `cli.md` (generated)

`rigg dev cli-reference` walks the clap command tree and prints Markdown:
one section per command with usage line, description, arguments, options
(with defaults and env fallbacks), and subcommand links. A guard test in
`crates/rigg/tests/cli_surface.rs` asserts `docs/reference/cli.md` equals
the generated output (same pattern as the CONCEPTS.md crate-copy guard).
The command is documented in `CLAUDE.md` as the way to refresh the page.

### 3.8 `environment-variables.md`

Every `RIGG_*` and `AZURE_*` variable read by any crate (enumerated by
`grep -rho 'RIGG_[A-Z_]*\|AZURE_[A-Z_]*' crates/*/src`), with purpose,
default and which are test-only. A unit test asserts the page mentions every
variable the code reads.

### 3.9 `exit-codes-and-questions.md`

Exit codes 0–6 with the situations that produce each; the `needs-input`
JSON protocol, `--answer`, `--answers-file`, `RIGG_NON_INTERACTIVE`, `--yes`
semantics (never satisfies the protected gate), and every question-id
prefix from `ask::KNOWN_ID_PREFIXES` with the ids each command asks. A test
asserts every prefix in `KNOWN_ID_PREFIXES` appears in the page.

## 4. Generated pages and guards

| Page | Generator | Guard |
|---|---|---|
| `docs/reference/cli.md` | `rigg dev cli-reference` | `cli_surface.rs::cli_reference_is_current` |
| InfraRef table in `resource-files.md` (between `<!-- infra-table:start/end -->` markers) | `rigg dev infra-table` | `cli_surface.rs::infra_table_is_current` |
| MCP tool list in `MCP.md` (between markers) | `rigg mcp tools --markdown` | `cli_surface.rs::mcp_tool_table_is_current` |
| `CONCEPTS.md` ↔ `crates/rigg/CONCEPTS.md` | (copy) | existing guard |

The generators are `dev` subcommands so they never appear in user tutorials.
Guards compare normalised text (trailing whitespace trimmed) and print a
`cargo run -q --bin rigg -- dev <x> > <file>` hint on failure.

## 5. Tutorials

Each tutorial is runnable top to bottom, states prerequisites and cost up
front, shows the exact expected output of every command (trimmed, from a
real run), explains *why* after each step in two or three sentences, and
ends with "what you have now" and "clean up". Commands use `rigg` from
PATH; the reader is assumed logged in with `az login`.

1. **Pull an existing solution** (rung 1 of the definition-of-done ladder):
   `rigg init` in an empty directory, discovery, `rigg new project`,
   `rigg adopt <project> all`, `rigg env learn`, `rigg status`, commit;
   then the round trip: delete a resource in the portal or with
   `rigg delete … --remote`, `rigg push`, `rigg verify`. Demonstrates the
   bindings cache and the auth preflight.
2. **Build from scratch**: `rigg new pipeline` (blob → index → indexer →
   knowledge source → knowledge base), `--identity`, `rigg auth doctor
   --fix`, push, `rigg az indexer run --watch`, `rigg az knowledge-base
   ask`, then add a Foundry agent with `x-rigg-ref`, push, `rigg az agent
   ask`.
3. **Add an environment and promote**: `rigg env add staging`, `rigg env
   bind`, `rigg promote <project> --from dev --to staging` with the rewiring
   preview and the binding questions, non-interactive form with
   `--answer`, doctor on the new env, push, verify.
4. **Push to protected production**: `policy: protected`, `strict-bindings`,
   `--confirm-env`, what `--yes` does not do, `push --verify`, `rigg auth
   roles list`, CI with `rigg ci init` and a service principal, and the
   `needs-input` loop from an agent (MCP).

`how-rigg-works.md` is the connective narrative (sync classes and
baselines; the binding layer and the InfraRef table; the identity
requirement graph; promote as translation; the question protocol) written
for a reader who has done tutorial 1.

## 6. Validation of the docs themselves

- Every tutorial is executed once, in order, against Kristofer's
  subscription in a throwaway workspace directory under `e2e-test/` (never
  committed): tutorial 1 and 3 against the existing `regulus` project and
  `mklabsrch`/`mklabaifndr`; tutorial 2 creates resources with a `tut-`
  prefix inside those services and deletes them at the end; tutorial 4 uses
  a `prod` environment pointing at the same services with `protected: true`
  and stops at the dry-run + `--confirm-env` demonstration (no second
  service is created). No new billable Azure resources; indexer runs on the
  small regulatory corpus are acceptable.
- Every command block in `docs/**/*.md` and `README.md` whose first token
  is `rigg` is parsed by a test (`crates/rigg/tests/docs_commands.rs`) that
  invokes `rigg <args> --help`-equivalent parsing through clap
  (`Cli::try_parse_from` with a `--dry-parse` guard) to prove the
  subcommand and flags exist. Placeholders in angle brackets are
  substituted with dummy values. This is the mechanical "docs never claim a
  flag that does not exist" check.
- A link checker test asserts every relative Markdown link in the docs tree
  resolves to a file/anchor.
- The existing `test-complete-enduser-experience` skill is rewritten to
  walk the four tutorials (issue #6 acceptance).

## 7. Root files after the change

- `README.md`: problem, what rigg does, install, 12-line quick start, a
  "Documentation" section linking the four tutorials and the reference
  index, exit codes table (kept, it is short), license. Feature prose moves
  to `how-rigg-works.md`; the command table moves to `cli.md`.
- `GETTING_STARTED.md`: kept as a filename people may have bookmarked;
  contents become the docs index pointer.
- `CHANGELOG.md`: "Docs" subsection under 2.0.0 listing the new tree and
  the generated pages.
- `CLAUDE.md`, `.claude/skills/rigg-guide/SKILL.md`: point at the reference
  pages; add the three generators to the "keeping docs current" list.

## 8. Out of scope / deferred

- Translated docs; a docs website (the Markdown tree renders on GitHub).
- Screenshots of the Azure portal.
- Per-kind JSON schema pages beyond the InfraRef table (schema validation
  already gives exact errors).
