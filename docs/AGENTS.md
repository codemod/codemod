# Documentation Rules

Docs are Mintlify MDX. Follow the existing navigation in `docs/docs.json` and the writing style in
`codemod-docs` skill.

## Content

- Write task-oriented docs that start with the user's goal and expected outcome.
- Keep terminology consistent with the CLI, JSSG, workflow, registry, and platform surfaces.
- Use Mintlify components only when they clarify the page: `Note`, `Tip`, `Warning`, `Info`, `Check`,
  `Steps`, `Tabs`, `AccordionGroup`, `RequestExample`, and `ResponseExample`.
- When adding or moving pages, update `docs/docs.json` in the same change.
- Verify code snippets against current CLI names and package paths.

## Mintlify vs Wish skill-docs (MECE)

This tree is **human product documentation** only.

| Corpus | Location | Audience | Update when |
|--------|----------|----------|-------------|
| **Product docs (this tree)** | `docs/` in this repo | Humans | User-facing product/CLI behavior, concepts, how-tos |
| **Wish skill-docs** | `codemod-app` → `packages/modern-ai/src/global-chat/skill-docs/` | Wish (`load_skill`) | In-app agent SDK/tools, agent policy, embedded schemas |

- Do **not** document Wish internals here: `load_skill`, `execute` / `execute_server`, page SDK
  method lists, Zod dumps for chat tools, or agent consent/policy rules.
- A short product overview of Wish (what it is, when to open it) is fine — e.g.
  `docs/enterprise/codemod-wish.mdx`.
- Same product topic may appear in both corpora; keep concepts/how-tos here and agent contracts in
  `codemod-app` skill-docs. Do not paste skill-docs into Mintlify or Mintlify pages into skill-docs.

## End of development

After shipping a user-facing CLI/platform change in this repo:

1. Update the matching Mintlify pages (and `docs/docs.json` if needed) in the same effort when
   practical.
2. If Wish / in-app agent contracts also changed, **heads-up the user** to update skill-docs in
   `codemod-app` — do not silently skip.
3. If no docs apply, say so briefly.

Heads-up example:

> **Docs follow-up (`codemod-app` repo):** Update
> `packages/modern-ai/src/global-chat/skill-docs/` (and `skillRegistry` if needed). Open
> `codemod-app` or ask there so `wish-skill-docs` can be applied.
