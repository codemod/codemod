# Agent alone vs agent + Solid migration assistant: codeimage, Solid 1 to 2

Same agent, model, prompt, commit (codeimage 27b185f) and $24 cap per run; 3 fresh runs per arm, Nov 17 2026.
Assistant arm also had `solid-migration-assistant` (battle-test-fixes c1db086). Scope: core Solid only;
libraries with no Solid 2 release left alone.

## Medians

| Arm | Wall time | Cost | tsc runs | Reached finish line |
|---|---|---|---|---|
| Agent alone | 23.1 min | $9.02 | 56 | 2 of 3 |
| Agent + assistant | 19.7 min | $7.35 | 37 | 1 of 3 |

## Runs

| Run | Wall | Cost | Tool calls | tsc runs | Core errors left | Finished |
|---|---|---|---|---|---|---|
| alone-1 | 36.6 min | $9.10 | 853 | 64 | 0 | yes |
| alone-2 | 22.8 min | $9.02 | 818 | 40 | 0 | yes |
| alone-3 | 23.1 min | $7.79 | 812 | 56 | 6 (packages/ui Dynamic typing) | no |
| assistant-1 | 19.7 min | $9.25 | 774 | 32 | 5 (packages/ui Dynamic typing) | no |
| assistant-2 | 16.1 min | $7.35 | 687 | 37 | 0 | yes |
| assistant-3 | 30.2 min | $7.27 | 709 | 53 | 6 (packages/ui Dynamic typing) | no |

All six moved every package to solid-js 2.0.0, installed, and built the libraries. No budget cap was hit.

## Grading notes
- "Core errors left" excludes 15 errors already present on Solid 1 and errors caused by libraries with no
  Solid 2 release. Eight errors appear in every run (CanvasEditor, FeatureCheckBanner, PresetSwitcher,
  FontPickerListbox, ImagePickerList, TabName, index.tsx); each comes through a local wrapper around
  Kobalte, solid-toast, @codeui/kit or statebuilder, so they are counted as library errors here. The
  automatic grader (grade.py) only catches direct imports, so its grade.json files still list them as core.
- The agents' prompt missed three libraries with no Solid 2 release (statebuilder, @codeui/kit,
  @corvu/utils). Same prompt in both arms.
- Type silencing in diffs: alone-1 one ts-ignore/expect and one `: any`; assistant-2 one `as any`; others none.
- A first launch was discarded (permission setup let some edits fail); it is in invalid-first-launch/.

## Read
With three runs per arm the assistant arm was somewhat faster and cheaper, with fewer type-check loops,
but it did not finish more often; the spread inside each arm is as large as the gap between them.
