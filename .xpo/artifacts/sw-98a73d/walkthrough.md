# Walkthrough — Removing jj colocation from smallworld

## What this was

The repo was a **colocated jj repo**: a normal `.git` directory with a `.jj` directory
beside it, kept in sync by a background `jj branching server` process belonging to the
VisualJJ VS Code extension. Nobody was typing `jj` commands — the extension snapshotted the
working copy continuously on its own.

Colocation means jj mirrors its view of the working copy into git's index. jj auto-tracks
new files (`snapshot.auto-track` defaults to `all()`), and the only way to represent
"git knows this path exists but has no content for it" is an **intent-to-add** entry
(`git add -N`): the path appears in the index with mode `000000` and a null object ID.

## Why it had to go

Intent-to-add entries make one very ordinary command destructive:

```
git checkout -- crates/engine/src/lib.rs
```

`git checkout -- <path>` means "restore this path from the index". For an intent-to-add
path the index holds *nothing*, so git faithfully restores nothing — the file is truncated
to zero bytes. This happened during sw-10ef86 while reverting a deliberately broken probe
edit, and cost a rewrite of `lib.rs`.

`git status --short` reports these as ` A`, which reads like "staged addition" and is easy
to mistake for a normal staged file. `git status --porcelain=v2` is the honest view:

```
1 .A N... 000000 000000 100644 0000000000000000 0000000000000000 crates/engine/src/lib.rs
                 ^^^^^^        ^^^^^^^^^^^^^^^^
                 no index mode, null OID — nothing to restore from
```

The alternative fix was `snapshot.auto-track = "none()"`, which stops the export. The user
rejected it: it makes every new file require a manual `jj file track`, and until you run it
the file has no jj snapshot either — trading a papercut for a worse failure mode. jj was
providing no value here (nobody was using it directly), so it was removed outright.

## What was done

The user deleted `.jj/` and uninstalled the extension. That leaves state behind in `.git`,
which is what this issue covered:

1. **`refs/jj/keep/*` — 114 refs.** jj pins every snapshot it takes with a keep-ref so its
   operation log can reach them. With `.jj` gone these are unreachable garbage that
   nonetheless keep their objects alive forever, because refs are roots.

   ```
   git for-each-ref --format='delete %(refname)' refs/jj | git update-ref --stdin
   ```

   `update-ref --stdin` applies the whole batch in one transaction — a `for` loop over
   `git update-ref -d` would be 114 lock/unlock cycles.

2. **`git reset` (mixed).** Drops the intent-to-add index entries so the scaffolding files
   show up as plain `??` untracked again. **Never `--hard` here** — there was a full
   uncommitted workspace scaffold in the working tree, and `--hard` would have deleted the
   tracked-file modifications outright.

3. **`git gc --prune=now`.** With the keep-refs gone the snapshot commits are unreachable
   and collectable. `.git` went **3.2 MB → 164 KB**.

## Verification

- `git for-each-ref` shows exactly two refs: `main` and the sw-10ef86 story branch, both
  still at `288c809`.
- Every scaffolded file is intact and non-empty (`lib.rs` 677 B, `assets.rs` 4718 B,
  `shaders.rs` 5136 B, `Makefile` 1520 B, …).
- `make ci` exits 0 — fmt, clippy, 11 tests, build, viewer smoke test.
- The original hazard, retried on the same path, now fails loudly instead of silently:

  ```
  $ git checkout -- crates/engine/src/lib.rs
  error: pathspec 'crates/engine/src/lib.rs' did not match any file(s) known to git
  ```

  That is the desired end state: an untracked file cannot be "restored" into oblivion.

## Notes for later

- One `jj branching server` process (pid 45807) was still alive at cleanup time, held by
  the not-yet-restarted VS Code. It cannot do damage with `.jj` gone, but if a `.jj/`
  directory or `refs/jj/*` refs ever reappear, the extension is back — that is a signal,
  not a normal change.
- `.jj/` was deliberately **not** added to `.gitignore`. The tool is removed, not merely
  hidden; an ignore entry would imply it is expected to be there.
- Nothing about this changes the xpo workflow: branches still come from `mcp__xpo__start`.
