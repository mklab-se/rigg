# rigg — Direction-neutral diff: side-by-side table, dual hints, neutral AI

**Date:** 2026-07-10
**Status:** Design — approved (user-directed)
**Workstream:** F (from live Regulus testing).

## Problem

`rigg diff` renders differences push-framed without saying so. Live repro: the
user upgraded Regulus to gpt-5.6-luna in the portal; diff printed
`model: was "gpt-5.6-luna", now "gpt-5.2-chat"` and the AI summary warned
about a "model downgrade" — because (a) `output.rs` uses temporal words
("was/now") for a spatial comparison, (b) `diff.rs` deliberately frames
old=remote/new=local ("report what pushing would change") without surfacing
that frame, and (c) the AI system prompt hardcodes "what pushing it would do".
A user in pull-mode reads the same data as a bug. Also: `metadata.modified_at`
(portal-maintained) pollutes every agent diff, and value previews say
"(1 keys)".

## Decisions (settled with the user)

- **Two-column table** per changed resource: `field | local | Azure (<env>)`.
  No temporal language anywhere.
- **Dual-direction hints** after drift — rigg never assumes intent, it explains
  both actions.
- **AI summary reframed neutral** and slimmed to interpretation (consequences
  of each direction, risks) — it must not re-narrate the table.
- **Keep table AND AI summary**: the table is deterministic ground truth; the
  AI is fallible interpretation. (User accepted the recommendation.)
- `metadata.modified_at` becomes an Agent volatile field.

## Design

### 1. Renderer (rigg-diff)

`format_report`/`format_text`/`format_markdown` gain explicit side labels:

```rust
pub struct SideLabels {
    /// Column for the diff's `new` side (local files in local mode).
    pub new_side: String,   // e.g. "local", or env A's name in compare mode
    /// Column for the diff's `old` side (the remote service in local mode).
    pub old_side: String,   // e.g. "Azure (dev)", or env B's name
}
```

Text format per resource:

```
regulus/agents/Regulus — differs (5 field(s))

  field                              local                Azure (dev)
  model                              "gpt-5.2-chat"       "gpt-5.6-luna"
  reasoning                          (absent)             {...} (1 key)
  metadata.microsoft.voice-live.enabled  (absent)         "false"
```

- Column 1 = `new_side` (local), column 2 = `old_side` (Azure) — matching the
  approved preview. Added/Removed render as values vs `(absent)`.
- Changes carrying a pre-set `description` (higher-layer array summaries)
  render as a full-width row under the field column.
- Long values stay truncated per `format_value_preview`; fix the "(1 keys)"
  pluralization while touching it.
- Markdown format becomes a Markdown table with the same columns (PR-friendly).
- JSON format unchanged.

### 2. Hints (rigg diff command)

After a drifted **local-vs-remote** diff in text format, print:

```
hint: rigg pull <project> — update local files to match Azure
      rigg push <project> — make Azure match your local files
```

Named per drifted project when one project drifted; `<project>` placeholder
when several. Suppressed in compare-env mode (neither action applies), in
`--format markdown`/`json`, and when there is no drift.

### 3. AI summary (ai_assist)

New system prompt, direction-neutral:

- Explain the differences between the LOCAL files and what is currently in
  AZURE, attributing values to the correct side (the report labels them).
- Do NOT assume the user intends to push or pull.
- Then two short sections: "If you pull: …" and "If you push: …" — one or two
  lines each, calling out risks (deletions, immutable index fields,
  SKU/capacity/billing) under the relevant direction.
- Do not restate every field; interpret. Max ~150 words.

### 4. Registry

Agent `volatile_fields` gains `"metadata.modified_at"`. (Portal rewrites it on
every edit; the rest of `metadata` — logo, voice-live config, etc. — remains
user-visible config.) Existing local files shed it on next pull/push
canonicalization; diff ignores it immediately.

### 5. Docs

README's semantic-diff sample block updated to the new table + hint shape.

## Testing

- rigg-diff unit tests: table rendering (modified/added/removed rows, labels
  in header, description rows, pluralization), markdown table shape.
- sync.rs: existing diff-format assertions updated to the new shape (this is
  the sanctioned breaking change); a hint-presence test on a drifted diff and
  hint-absence when clean.
- registry test: agent modified_at volatile.
- Live acceptance: re-run the user's exact scenario — portal-changed model —
  and confirm the table reads correctly in both mindsets and the AI summary
  no longer says "downgrade" unconditionally.

## Files touched

`crates/rigg-diff/src/output.rs`; `crates/rigg/src/commands/diff.rs`;
`crates/rigg/src/commands/ai_assist.rs`; `crates/rigg-core/src/registry.rs`;
`README.md`; tests inline + `crates/rigg/tests/sync.rs`.
