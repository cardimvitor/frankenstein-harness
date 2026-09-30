# Version sources and licences for the built-in packs

Checked 2026-09-30. Re-check before each release and before adding a pack.

## Versions

| Stack | Fact | Source |
|---|---|---|
| .NET 10 | LTS, released 2025-11-11, latest patch 10.0.12, supported until 2028-11-14 | Microsoft .NET support policy page (updated 2026-09-08) |
| .NET 9 | STS, released 2024-11-12, latest 9.0.20, **support ends 2026-11-10** | same |
| .NET 8 | LTS, released 2023-11-14, latest 8.0.31, **support ends 2026-11-10** | same |
| .NET 11 | RC1 published 2026-09-08; GA expected November 2026 (odd number: STS) | same |
| React | latest 19.3.0 on npm; 19.0.0 published 2024-12-05; 18.0.0 published 2022-03-29 | npm registry |
| Angular | latest 22.2.0 on npm; 17.0.0 2023-11-08, 18.0.0 2024-05-22, 19.0.0 2024-11-19, 20.0.0 2025-05-28, 21.0.0 2025-11-19 | npm registry |
| AngularJS | latest 1.8.3 (end of life) | npm registry |
| TypeScript | latest 7.0.2 (relevant to the fingerprint and verify commands) | npm registry |

What this changes for the packs:

- `dotnet-8-10` now also applies to .NET 11 projects (`versions: 8,9,10,11`) and states which C# version each target framework defaults to.
- Both .NET 8 and .NET 9 lose support on 2026-11-10, weeks from now: a project on either should be flagged to its owner in the planning notes, not silently migrated.
- `angular-17plus` covers 17 through the current 22; it notes that components are standalone by default from Angular 19.
- .NET Framework 4.8 and AngularJS 1.x are out of mainstream support; those packs prefer minimal, safe changes.

## Licences of the pack content

- The built-in packs are **original text written for this project**. No documentation from Microsoft, the React team, the Angular team or Apple was copied, so no CC-BY attribution is needed today.
- The HIG-inspired pack paraphrases widely known design principles (hierarchy, consistency, contrast ratios, target sizes, motion preferences) and reproduces no Apple text or assets. Apple's terms were not reviewed line by line; keep it a paraphrase, and do not add Apple text or images without checking the current terms.
- If a future pack quotes official documentation, check that page's licence first (much of Microsoft Learn and MDN-style content is CC-BY-4.0 or MIT-licensed and requires attribution) and add the attribution to this file.

## Process for a new or updated pack

1. Write the guidance in your own words, as short bullet points a senior engineer would agree with.
2. Verify every version-specific claim against an official source and record it in the table above.
3. Set `versions:` in the frontmatter so the pack only applies to matching stacks; add a fingerprint test.
4. Data-only rule: no commands, URLs, secrets or instructions about permissions (the store's validator applies to learned skills; built-ins follow the same rule by review).
