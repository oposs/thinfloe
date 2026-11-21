# Dev pace: faster rounds from branch to main

Date: 2026-11-21. Owner approved all six proposals on 2026-11-21; the
controller made the calls marked "controller's call".

## Intent

A feature round (several branches, 2026-11-20) cost about four hours of
waiting between the first push and the last merge. The tests themselves are
not slow; the time goes into merging PRs one after the other (every merge
makes the next PR out of date, and `local / image` takes 29-33 min per
run), repeated local gates after each rebase, and review loops that fix
every minor finding. The goal is a shorter round without weaker checks on
what reaches `main`.

## Goals

1. One CI run per round instead of one per branch.
2. Fewer local gate runs: the full set once, a short set after a rebase.
3. A shorter `local / image` job.
4. Faster local tests, and a menu-only target that needs no image build.
5. Tests that do not break when another branch adds a struct field.
6. A review loop with a fixed end.

## Non-goals

- No change to the repo-infra ruleset standard: `strict` up-to-date checks
  stay on, no merge queue (controller's call: both are standard-wide
  changes, and an integration branch makes `strict` cheap).
- No change to what the tests check, and no test removed. The smoke tests
  and `test-runtime` still run on every PR.
- No change to `release-build-local.yml` in this round (it runs the same
  smoke tests; it can follow the CI split later if that works).
- `CHANGES.d/` fragments: this waits for oposs/repo-infra#172. Nothing in
  thinfloe until that lands.

## Anti-goals

- A green `ci-passed` on a build that did not run every test. The
  `if: always()` logic in `ci.yml` and the list of jobs in `ci-passed`'s
  `needs:` must cover every new job.
- A CI job that silently rebuilds the container instead of using the one
  built in the job before it (then the split costs time instead of saving
  it, and tests may run on a different build).
- More than 4 cores used locally.

## Measurements (run 44822629521, `local / image`, 32 min)

| Step | Time |
|------|------|
| runner setup | 0:13 |
| `test-unit` inside `make test` (also run by `local / unit`) | ~4:45 |
| container build (labwc 1:20, menu 3:00, plymouth art 0:10, rootfs 2:50) | ~7:40 |
| rootfs export, mksquashfs | ~2:20 |
| integration tests | ~2:00 |
| `make image` | 0:11 |
| nine smoke tests, in sequence (upgrade 3:49, two-sticks 1:18, write 1:14) | ~9:40 |
| `test-runtime` | 4:57 |

## The parts

### 1. Integration branch per round (practice, in CLAUDE.md)

For a round with more than one feature branch: the branches are rebased in
turn onto one `round/<date>` branch, the full local gates run once on that
branch, and one PR goes to `main`. A branch that is ready alone and has no
round open still goes as its own PR. The rebase merge onto `main` keeps
every branch's commits.

### 2. Gate rules (CLAUDE.md, and memory)

- Before the first push of a branch: the full local gates (`make test-unit`,
  `make test`, `make test-runtime` where the change touches the session or
  the menu, and the smoke tests the change touches).
- After a rebase with no conflict outside `CHANGES.md`: only
  `cargo clippy --all-targets` and `cargo test` in `src/thinfloe-menu`; CI
  does the rest.
- After a rebase with a code conflict: the full gates again.

### 3. Review rule (CLAUDE.md, and memory)

One whole-branch review, one fix wave, one scoped re-review of the fixes.
After that, a finding goes to a follow-up list in the PR body unless it is
a real bug (wrong behaviour, data loss, security, a broken constraint).
This replaces the old rule "fix every minor".

Controller's call for a round: the loop runs per feature branch, before
the branch joins the round branch (the reviews of several branches can run
at the same time, and each diff stays one coherent change). The round
branch gets no second whole review; only conflict resolutions made during
the rebases are checked. The follow-up lists go into the round PR's body,
one heading per branch; the controller offers them to the owner after the
merge, as with any other noticed defect.

### 4. CI: no duplicate unit run

`make test` depends on `test-unit`. Add a target (controller's call:
`test-integration`, the current `test` recipe without the `test-unit`
prerequisite; `test` becomes `test-unit test-integration`) and let the
image job call `make test-integration`. Locally `make test` stays as it
is. Expected saving: about 4:45.

### 5. CI: smoke tests and runtime tests in parallel jobs

The `image` job builds the container, exports the rootfs, runs the
integration tests and `make image`, uploads `artifacts/rootfs/` and
`artifacts/images/` as an actions artifact, and then runs `test-runtime`
itself. The smoke tests run in parallel jobs that download the artifact.
Controller's call: `test-runtime` stays in the `image` job because it is
the only step that needs the `thinfloe-ubuntu` podman image (as the base
of the runtime-test image); moving it would mean `podman save`/`load` of a
multi-GB image, which eats the saving. The smoke tests need only files.
Grouping, by measured time: (a) `qemu-smoke-upgrade`; (b)
`qemu-smoke-two-sticks`, `-write`, `-grow`; (c) `qemu-smoke`, `-netboot`,
`-addons`, `-timezone`, `-timezone-netboot`. The builder may change the
groups after measuring.

The hand-over is the hard part:

- `make` in the smoke jobs must not build the container or export the
  rootfs again (there is no podman image there). The stamp
  `.ubuntu-container` holds the image ID and an input hash; the smoke
  targets need a way to run on the downloaded files without their build
  prerequisites. How is left to the builder. The guard is a check in each
  smoke job that fails if a `thinfloe-ubuntu` container build or a rootfs
  export starts (not any `podman build`: `test-runtime` builds its own
  test image legitimately), so a silent rebuild turns the job red.
- `ci-passed` in `ci.yml` lists `local` only; the new jobs live inside
  `ci-local.yml`, so a failure in any of them fails `local`. Check that
  this holds for jobs with `needs:` inside a called workflow.
- The failure log upload (`artifacts/images/*.log*`) moves to each smoke
  job.

Measurement (controller's call): part 4 goes to `main` first as its own
PR; its CI runs on `main` give the baseline. Part 5 is judged on two CI
runs of its PR against two baseline runs, `local` time from the first job
start to the last job end. Expected: about 32 → 27 min after part 4, about
27 → 19 min after part 5 (the smoke chain of 9:40 becomes the longest
group, ~4 min, plus ~1.5 min for setup and download, and it overlaps
`test-runtime`). Keep part 5 if the mean drops by at least 4 min; else
revert it and record the numbers in `docs/release-ci.md`.

### 6. CI: podman layer cache (not in this round)

Controller's call: not built in this round, whatever the numbers. If
`local` still takes more than 20 min after parts 4 and 5,
`docs/release-ci.md` records the measured time and the recommendation
(actions cache of the podman storage, within the 10 GB repo limit, or a
registry cache, which needs `packages: write` in a repo-infra caller and so
goes through repo-infra first).

### 7. Local: parallel Python unit tests

`make test-unit` runs the 8 `test_*.py` modules of `test/unit/` with at most
4 processes at once (one `python3 -m unittest -v <module>` per module, no
new dependency), keeps each module's output together, and fails if any
module fails. Controller's call: a small script under `scripts/` with
`$(TEST_THREADS)` as the process count. `cargo test` follows the Python
tests as today.

### 8. Local: `make test-menu`

`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` (with the
flags the project uses today, if they differ) and `cargo test` in
`src/thinfloe-menu`, with `BUILD_JOBS` and `TEST_THREADS`. No container, no
image. `test-runtime` needs the rootfs and stays out.

### 9. Test fixtures with `..Default::default()`

Struct literals in the menu's tests that list every field (the case that
broke: `netdetail::Details` and its `time` field) use
`..Default::default()` for the fields the test does not care about. Where a
struct has no `Default`, add a derive if every field has one; if a field
cannot have a sensible default, leave that struct alone. Production code is
not changed. Scope: the test modules and `tests/`; the builder lists which
structs it touched.

## Docs

- CLAUDE.md: parts 1-3 as short rules under "Rules that are easy to
  break" or a new short "Workflow" section; the new make targets in
  "Commands".
- `docs/release-ci.md`: the new CI split and the measured times, before and
  after.
- CHANGES.md: no entry for parts 1-3 and 9 (nothing users see). Parts 4-8
  change only developer targets; the controller's call is no entry either,
  since CHANGES.md readers are users and administrators.

## Left to the builder

- How the downstream jobs avoid the rebuild (section 5).
- The exact smoke grouping after the first measured run.
- The parallel runner script's name and output format.
- Which fixture structs get `..Default::default()`.
