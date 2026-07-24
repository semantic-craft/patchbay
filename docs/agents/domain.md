# Domain Docs

How the engineering skills should consume this repo's domain documentation when exploring the codebase.

## Before exploring, read these

- **`CONTEXT.md`** at the repo root, if it exists.
- **`docs/adr/`** for ADRs that touch the area being changed.

If either location does not exist, proceed silently. Do not suggest creating empty domain documents upfront. The domain-modeling workflows create them lazily when terms or decisions are actually resolved.

## File structure

Patchbay is a single-context repository:

```text
/
├── CONTEXT.md
├── docs/adr/
└── src/
```

## Use the glossary's vocabulary

When output names a domain concept in an issue title, refactor proposal, hypothesis, or test name, use the term as defined in `CONTEXT.md`. Do not drift to synonyms the glossary explicitly avoids.

If a needed concept is absent from the glossary, first reconsider whether the term is being invented unnecessarily. If it represents a real gap, note it for the domain-modeling workflow.

## Flag ADR conflicts

If an output contradicts an existing ADR, surface the conflict explicitly rather than silently overriding it.
