# avatar

2026-07-23 — golden pair via CLI; migrated the avatar wrapper family to Base UI without changing local size variants.

## Changed

- `web/console/src/components/ui/avatar.tsx:2` now imports the Base UI avatar primitive.
- `web/console/src/components/ui/avatar.tsx:9` uses Base UI part prop types for the root and keeps the project's `sm`/`default`/`lg` sizing contract.
- `web/console/src/components/ui/avatar.tsx:24` and `web/console/src/components/ui/avatar.tsx:38` migrate image and fallback parts to their Base UI prop types.
- Leftover scan clean: `grep -n "radix-ui\|@radix-ui" web/console/src/components/ui/avatar.tsx` returned no matches.

## Left alone

- `web/console/src/app/layouts/console-layout.tsx` uses only `Avatar` and `AvatarFallback`, so no consumer change was required.

## Behavior changes

- The fallback delay prop is now named `delay` instead of Radix's `delayMs`. The repository had no call sites using the old prop.

## Verify by hand

- Open the account menu and confirm initials render in the avatar fallback.
- Test a valid and broken image URL and verify the fallback transition and sizing.
