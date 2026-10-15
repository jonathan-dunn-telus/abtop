# Remix fork: releases and downstream syncs

How to release this fork and how to merge new work from the repo it tracks.
These rules replace the *Release Process* section of `AGENTS.md` in this fork.

| Name | Repo | Git remote | Branch |
|---|---|---|---|
| **Downstream** | `graykode/abtop` | `origin` | `main` |
| **Fork** | `raphberube/abtop` | `raphberube` | `remix` (default branch) |

## Versioning

Versions look like `X.Y.Z-remix.N`.

- `X.Y.Z` is downstream's version, taken from the `version` field of downstream's `Cargo.toml`.
  It changes **only** when a downstream sync brings in a new downstream version.
- `N` counts fork releases on that base. `-remix.0` marks the unreleased downstream base and is
  never tagged. The first fork release on each base is `-remix.1`.

[release-please](https://github.com/googleapis/release-please) computes the next version from
conventional commits on `remix` (`release-please-config.json`, `versioning: prerelease`):

| Commits since the last release | Next version |
|---|---|
| `fix:`, `feat:`, `perf:`, `revert:` | `X.Y.Z-remix.N+1` |
| Only `chore:`, `docs:`, `refactor:`, `ci:`, or merge commits | No release PR |
| A `Release-As: <version>` footer | Exactly `<version>`. The newest one wins |
| `feat!:`, `fix!:`, or a `BREAKING CHANGE:` footer | **`X.(Y+1).0-remix.N`. This changes the base, so never use them on `remix`** |

## Merging feature branches into `remix`

release-please reads the message of **every** commit on `remix`, including commits that reach it
through a merge. It ignores the `Merge branch 'x' into remix` commit itself.

1. Every commit on the feature branch must use a conventional message (`AGENTS.md` → *Commit
   Convention*). Its message becomes a changelog entry.
2. release-please walks `remix` history from newest to oldest and stops at the last release
   commit. A branch commit dated **before** the last release can be listed after that commit
   and silently dropped from the release. Before merging, list any such commits:

   ```bash
   git fetch raphberube --tags
   last=$(git log -1 --format=%ct "$(git describe --tags --abbrev=0 --match 'v*-remix.*' remix)")
   git log --format='%ct %h %s' remix..feat/x | awk -v t="$last" '$1 < t'
   ```

   Before the first fork release there is no `v*-remix.*` tag yet. In that case the cutoff is
   `bootstrap-sha` in `release-please-config.json`.

   If this prints anything, give the branch's commits fresh dates without changing their base,
   then force-push the branch:

   ```bash
   git switch feat/x
   git rebase --force-rebase "$(git merge-base feat/x remix)"
   ```

3. Merge with a merge commit and push:

   ```bash
   git switch remix
   git merge --no-ff feat/x
   git push raphberube remix
   ```

## Cutting a release

1. Every push to `remix` runs `.github/workflows/release-please.yml`. It opens or updates a
   release PR from `release-please--branches--remix` into `remix`. That PR bumps `Cargo.toml`,
   `Cargo.lock`, `.release-please-manifest.json`, and `CHANGELOG.md`.
2. Review the version and changelog in the PR, then merge it. CI does not run on this PR,
   because GitHub does not start workflows for PRs opened with `GITHUB_TOKEN`.
3. release-please tags the merge commit `vX.Y.Z-remix.N` and creates the GitHub Release. GitHub
   marks it as a prerelease because of the `-remix.N` suffix, so `/releases/latest` never points
   to it. Link to releases by tag.
4. The `build` job in the same workflow builds `abtop` for each target in its matrix. It uploads
   `abtop-<tag>-<target>.tar.gz` (`.zip` on Windows) and a matching `.sha256` file to the
   release. If a target fails, re-run the failed job from the Actions tab. Uploads replace any
   files that already exist.
5. Update your local copy: `git switch remix && git pull raphberube remix && git fetch raphberube --tags`.

The first run needs **Settings → Actions → General → Allow GitHub Actions to create and approve
pull requests** turned on, or release-please cannot open its PR.

**Do NOT edit the `version` in `Cargo.toml` by hand.** The release PR owns it.
**Do NOT create or push `v*` tags by hand**, including `git push raphberube --tags`. That command
would also push every downstream tag fetched from `origin`.
**Do NOT run `cargo publish` or `gh release create`.** The `abtop` crate on crates.io belongs to
downstream.

## Syncing downstream

1. Fetch downstream and compare its version with the fork's base:

   ```bash
   git fetch origin
   git show origin/main:Cargo.toml | grep -m1 '^version'
   git show remix:Cargo.toml | grep -m1 '^version'
   ```

2. Merge `origin/main` into `remix`, following the [merge rules](#merge-rules) below.
   - If downstream's `X.Y.Z` differs from the fork's base, add a `Release-As` footer to the
     merge commit:

     ```bash
     git switch remix
     git merge --no-ff origin/main -m "chore: sync downstream vX.Y.Z" -m "Release-As: X.Y.Z-remix.1"
     ```

   - If the version is the same, merge without the footer. The base stays the same and
     downstream's `feat:`/`fix:` commits bump `N`.

3. Resolve conflicts, then run `cargo clippy -- -D warnings && cargo test`.
4. Push to `raphberube remix`. The next release PR carries the new base. If no release PR
   appears, there are no user-facing commits yet. The `Release-As` footer stays in effect until
   the next release.
5. Check whether downstream added a workflow (see [Workflows](#workflows)).

### Merge rules

Downstream changes **MUST NEVER overwrite** the following fork content. When a merge conflicts
here, keep the fork's side and then add downstream's change by hand where the rule allows it.

| Content | Rule |
|---|---|
| `.github/workflows/ci.yml` | Keep `branches: [remix]` and every `uses:` pinned to a full commit SHA with a `# <tag>` comment. Downstream's new steps may be added, with any new action pinned to a SHA too. |
| `.github/workflows/release-please.yml`, `release-please-config.json`, `.release-please-manifest.json`, `CHANGELOG.md` | Fork-only. Keep them as they are, with `release-please.yml`'s `uses:` pinned to a full commit SHA like `ci.yml`. |
| `version` in `Cargo.toml` | Keep the fork's `X.Y.Z-remix.N`. The new base arrives through the `Release-As` footer and the release PR, not through the merge. |
| `Cargo.lock` | Take downstream's dependency changes, then run `cargo build` so the `abtop` entry matches the fork's `Cargo.toml` version again. |
| `on:` block of `.github/workflows/publish.yml` and `.github/workflows/release.yml` | Keep the fork's `workflow_dispatch`-only block and its `remix fork` comment. Downstream's changes to the rest of these files may be merged. |
| `docs/remix-release.md` and the pointer to it at the top of `AGENTS.md` | Fork-only. Keep them. |
| *Remix features* and *Installing the remix build* sections of `README.md`, `assets/remix/`, `scripts/remix-screenshots.sh`, `scripts/ansi_to_html.py`, `docs/remix-features.md` | Fork-only. Keep them; merge downstream's README changes around them. |
| Fork features in `src/`, the rest of `AGENTS.md`, and `docs/` | Combine both sides by hand. Never resolve them by taking downstream's version. |

**Never** run `git merge -X theirs`, `git checkout --theirs <path>`, or `git restore --theirs <path>`
on any path above.

## Workflows

| Workflow | File | State in the fork |
|---|---|---|
| CI | `ci.yml` | Enabled. Runs on pushes and PRs to `remix` |
| release-please | `release-please.yml` | Enabled. Runs on pushes to `remix`, and builds release binaries when it creates a release |
| Publish to crates.io | `publish.yml` | **Disabled, no automatic triggers. MUST NEVER be enabled** |
| Release | `release.yml` | **Disabled, no automatic triggers. MUST NEVER be enabled** |

- **`publish.yml`**, as downstream ships it, runs `cargo publish` on every `v*` tag, including `vX.Y.Z-remix.N`. That
  publishes downstream's `abtop` crate.
- **`release.yml`** is downstream's cargo-dist pipeline (`dist-workspace.toml`). As downstream
  ships it, it runs on every PR. On a tag it creates a GitHub Release, which clashes with release-please's release, and it
  pushes a Homebrew formula to `graykode/homebrew-tap`.

Each one has two safeguards. It is disabled in GitHub's Actions settings, and its `on:` block is
reduced to `workflow_dispatch`, so nothing triggers it even if it gets enabled again. The files
are kept rather than deleted, so downstream's edits outside the `on:` block still merge. Keep
the fork's `on:` block (see [Merge rules](#merge-rules)).

After every downstream sync, list the workflows. Disable any new one that publishes, releases,
or writes to a downstream-owned repo, and reduce its `on:` block to `workflow_dispatch` the same
way:

```bash
gh workflow list -R raphberube/abtop --all
gh workflow disable "<name>" -R raphberube/abtop
```

GitHub treats a renamed workflow file as a new, enabled workflow. If downstream renames
`publish.yml` or `release.yml`, give the renamed file the same two safeguards.
