# Automount follow-ups (Branch D) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the four follow-ups of PR #58: a stale write hold is removed without the hold file ever going missing, "Eject all" gives one OSD note for the batch, a card swapped for one of the same name gives a "connected" note, and the two-sticks ruling is written down.

**Architecture:** The hold directory gets an flock that both parties take: `thinfloe-upgrade` when it renames its hold into place, `thinfloe-automount` when it deletes a stale one. The stale delete becomes an inode check and an unlink under that lock, with no rename aside. The event file gets an optional fourth field (the number of sticks of an "eject all" batch); the contract file gets a ninth field (a per-boot mount number), which the OSD's stick identity includes.

**Tech Stack:** Python 3 (root helper, writer, unittest), Rust (menu crate `src/thinfloe-menu`, OSD), shell smoke tests in QEMU.

**Spec:** [docs/superpowers/plans/2026-11-26-round-fix-everything.md](2026-11-26-round-fix-everything.md), section "Branch D: fix/automount-followups" (rows D1 to D4). Background: PR #58 body ("Follow-ups"), [docs/notes/automount.md](../../notes/automount.md).

## Deviations from the round spec

- **D1, "put back with an atomic rename so the file is never missing":** a rename back cannot close the gap. Whatever the second call is (`link` today, `rename` or `renameat2(RENAME_NOREPLACE)`), the first rename has already taken a fresh hold away, and an `add` of another partition of the same stick can read the path in between (`cmd_add` calls `live_hold` outside the disk lock, `thinfloe-automount:882`). Closest right choice: no rename aside at all. The writer and the helper take one flock on the hold directory; under it the helper compares the inode and unlinks only the stale file. The file is never missing and no `.stale-<pid>` file is made.
- **D1, "the cleanup removes old `.stale-*`":** dropped. With no rename aside no such file is made, and `/run` is a tmpfs, so one left by the 4.13.0 helper is gone at the reboot that starts the new image.
- **D2, copy:** the spec quotes "<n> sticks can be removed." The note is "2 sticks can be removed", without a full stop, like the other stick notes ("MYSTICK can be removed").
- **D3, "event file carries a sequence number":** the event file has carried one since 4.12.0 (`seq<TAB>name<TAB>clean|unclean`), and it records only removals; a swap of two cards of the same name changes nothing the OSD compares in the contract file (`osd/notes.rs:71-75`, identity = disk + names). The number goes where the spec's parenthesis puts it, as a contract field appended at the end: a ninth field `mount` in `/run/thinfloe/removable`, a number the helper gives each mount, counted per boot. Readers ignore unknown trailing fields, so the contract stays compatible.

## Global Constraints

- `/run/thinfloe/removable` and `/run/thinfloe/removable.event` have one writer, `thinfloe-automount`; fields are only appended at the end; readers ignore unknown trailing fields (CLAUDE.md, Automount).
- A single (non-batch) event line stays exactly `seq<TAB>name<TAB>clean|unclean`: `scripts/smoke-automount-report.sh:47` (`event_is`) compares `cut -f 2-` with two fields.
- Every write of `thinfloe-upgrade` holds the stick and releases it before the first byte (CLAUDE.md, Automount); a hold whose pid lives and whose id matches is never deleted.
- The new hold-directory lock is a leaf: neither program takes another lock while holding it. It is held only around one `os.replace` (writer) or one `stat` plus `unlink` (helper), at most `HOLD_LOCK_WAIT = 5.0` s waited for.
- Machine: at most 4 cores (`BUILD_JOBS=4`, `TEST_THREADS=4`, the Makefile defaults); temporary files never in `/tmp` (the make targets set `TMPDIR` to `$(CURDIR)/.tmp` unless given).
- Commands: Python unit tests from `test/unit` as `python3 -m unittest -v <module>.<Class>[.<test>]`; menu tests as `cd src/thinfloe-menu && CARGO_BUILD_JOBS=4 systemd-run --user --scope -q -p MemoryMax=2G -- cargo test --locked -j 4 <filter> -- --test-threads=4`; fmt, clippy and all menu tests as `make test-menu`.
- English in code, comments and docs. CHANGES.md: the effect a user saw first, at most three sentences, no release header.
- Branch `fix/automount-followups`, cut from `round/2026-11-26b`. Commit messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. A writer killed while it holds the hold-directory lock: the flock goes with its fd, so the helper is never stuck. Pinned by `test_a_hold_lock_left_by_a_dead_writer_is_free` (Task 1).
2. No hold directory (an old image, a test without it): `_hold_drive` still returns None and logs, as today. Pinned by `test_no_hold_directory_is_no_hold` (Task 1).
3. "Eject all" with one of two sticks busy: one ordinary note for the stick that went, no count. Pinned by `test_eject_all_with_one_busy_stick_is_a_single_note` (Task 2).
4. A contract line from an older helper (8 fields) or with garbage in the ninth: mount 0, line kept. Pinned by `test_mount_field_is_optional` (Python) and `mount_field_is_optional_and_garbage_is_zero` (Rust), Task 3.
5. An event count field that is empty, 0, 1, garbage or larger than u32: the single-name note. Pinned by `event_count_is_optional_and_bounded` (Task 2).

---

### Task 1: The stale hold is deleted in place under a lock (D1)

**Files:**
- Modify: `overlay/usr/libexec/thinfloe/thinfloe-automount:595-641` (`live_hold`, `drop_stale_hold`), constants at `:397-402`, module docstring `:15-24`
- Modify: `overlay/usr/bin/thinfloe-upgrade:2535-2559` (`_hold_drive`), constant next to `AUTOMOUNT_HOLD_DIR` at `:199`
- Test: `test/unit/test_automount.py:861-923` (class `FollowUps`)
- Test: `test/unit/test_upgrade_batch.py:3377-3387` (class `TestAutomountedDrives`, line 3322)

**Interfaces:**
- Produces (helper): `HOLD_LOCK_WAIT = 5.0`; `class DirLock` with `__init__(self, path: str, timeout: float)`; `__enter__` opens `path` with `os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC` and polls `fcntl.flock(fd, LOCK_EX | LOCK_NB)` every 0.05 s like `FileLock.__enter__` (`:507-519`), raising `Busy` after `timeout`; `__exit__` closes the fd. `drop_stale_hold(path: str, ino: int) -> None` keeps its signature.
- Produces (writer): `HOLD_LOCK_WAIT = 5.0`; `_hold_dir_lock(folder: str, wait: float = HOLD_LOCK_WAIT) -> Optional[int]`: the locked directory fd, or None when `wait` ran out (the fd is closed then); `OSError` when the folder cannot be opened.

- [ ] **Step 1: Write the failing helper tests in `FollowUps`**

Delete `test_a_hold_written_between_the_check_and_the_delete_is_kept` (`:883-902`) and `test_a_hold_written_before_it_is_taken_aside_is_put_back` (`:904-923`): they drive the rename aside, which goes. Keep `test_a_hold_written_while_a_stale_one_is_read_is_kept` (`:861-881`) unchanged. Add, with the same fixture (`self.stick()`, stale hold `"999999\tsn-AA:1:1\n"` at `run/automount/hold/sdb`, `d = mod.read_dev("sdb1", mod.udev_props("sdb1", self.p), self.p)`):

```python
def test_a_stale_hold_is_deleted_in_place(self):
    # Never renamed: a fresh hold is never missing, nothing is left aside.
    with unittest.mock.patch.object(mod.os, "rename", side_effect=os.rename) as ren:
        self.assertFalse(mod.live_hold(self.p, d))
    self.assertFalse(hold.exists())
    self.assertEqual([c for c in ren.call_args_list if str(c.args[0]) == str(hold)], [])
    self.assertEqual(os.listdir(hold.parent), [])

def test_the_delete_waits_for_the_writers_lock(self):
    fd = os.open(hold.parent, os.O_RDONLY | os.O_DIRECTORY)
    fcntl.flock(fd, fcntl.LOCK_EX)               # the writer, mid-rename
    self.addCleanup(setattr, mod, "HOLD_LOCK_WAIT", mod.HOLD_LOCK_WAIT)
    mod.HOLD_LOCK_WAIT = 0.2
    self.assertFalse(mod.live_hold(self.p, d))
    self.assertTrue(hold.exists(), "kept while the writer holds the lock")
    os.close(fd)
    self.assertFalse(mod.live_hold(self.p, d))
    self.assertFalse(hold.exists())

def test_a_hold_replaced_before_the_lock_is_kept(self):
    # Wrap mod.DirLock.__enter__: os.replace a fresh hold
    # (f"{os.getpid()}\tsn-AA:2:2\n") into place, then take the real lock.
    self.assertFalse(mod.live_hold(self.p, d))
    self.assertEqual(hold.read_text(), fresh)
    self.assertEqual(sorted(os.listdir(hold.parent)), ["sdb"])

def test_a_hold_lock_left_by_a_dead_writer_is_free(self):
    # A child (sys.executable -c ...) flocks the directory, prints "locked",
    # sleeps 60; read the line, kill it, wait for it.
    self.assertFalse(mod.live_hold(self.p, d))
    self.assertFalse(hold.exists())
```

- [ ] **Step 2: Run them to see them fail**

Run: `cd test/unit && python3 -m unittest -v test_automount.FollowUps`
Expected: `test_a_stale_hold_is_deleted_in_place` FAILS (a rename of the hold is recorded), `test_the_delete_waits_for_the_writers_lock` errors (`AttributeError` on `HOLD_LOCK_WAIT`), `test_a_hold_replaced_before_the_lock_is_kept` errors (no `DirLock`).

- [ ] **Step 3: Implement `DirLock` and the new `drop_stale_hold`**

Under `DirLock(os.path.dirname(path), HOLD_LOCK_WAIT)`: `os.stat(path).st_ino == ino`, then `os.unlink(path)`; `FileNotFoundError` is "gone already". On `Busy`, log `f"stale hold {path} kept: its directory is locked"` and keep the file (the safe side: the stick stays unmounted until the next add). New docstring: the writer renames its hold in under the same lock, so the inode compared under it is the file the unlink removes. Module docstring (`:20-24`): one sentence, a hold file is put in place and deleted only under an flock of the hold directory.

- [ ] **Step 4: Run the helper's tests**

Run: `cd test/unit && python3 -m unittest -v test_automount`
Expected: OK, including `Add.test_stale_hold_is_removed_by_add`, `Add.test_hold_of_another_drive_is_removed_by_add` and `Eject.test_eject_removes_a_stale_hold`.

- [ ] **Step 5: Write the failing writer tests in `TestAutomountedDrives`**

Next to `test_hold_is_written_atomically_and_leaves_no_temp` (`:3377`), with its `DriveInfo` and `TF_AUTOMOUNT_HOLD` setup:

```python
def test_hold_is_renamed_in_under_the_directory_lock(self):
    fd = os.open(folder, os.O_RDONLY | os.O_DIRECTORY)
    fcntl.flock(fd, fcntl.LOCK_EX)               # the helper, mid-delete
    # ui._hold_drive(d) in a thread
    time.sleep(0.3)
    self.assertNotIn("sdb", os.listdir(folder), "waits for the helper's lock")
    os.close(fd)
    # join the thread (timeout 5)
    self.assertIn("sdb", os.listdir(folder))

def test_a_lock_that_stays_taken_delays_the_hold_by_the_wait_only(self):
    # lock held for the whole test; mock.patch.object(ui, "HOLD_LOCK_WAIT", 0.2)
    path = ui._hold_drive(d)
    self.assertEqual(os.listdir(folder), ["sdb"])   # written unlocked, logged

def test_no_hold_directory_is_no_hold(self):
    # TF_AUTOMOUNT_HOLD = a path under self.tmp.name that does not exist
    self.assertIsNone(ui._hold_drive(d))
```

Each test calls `ui._unregister_cleanup("file", path)` for a hold it made, as the existing test does.

Run: `cd test/unit && python3 -m unittest -v test_upgrade_batch.TestAutomountedDrives`
Expected: the first FAILS ("sdb" is there at once), the second errors (no `HOLD_LOCK_WAIT`), the third passes (it pins today's behaviour).

- [ ] **Step 6: Implement `_hold_dir_lock` and use it in `_hold_drive`**

Write the temp file as today; `lock = _hold_dir_lock(folder)`; when None, log `f"automount hold directory locked for {HOLD_LOCK_WAIT:g} s: hold written unlocked"`; `os.replace(tmp, path)`; close the lock fd in a `finally`. An `OSError` from opening the folder takes the existing `except OSError` path (temp unlinked, None). The unlink of the hold at the end of `fresh_install` (`:2596-2601`) stays unlocked: the helper deletes only the inode it read, and a vanished file is "gone already".

- [ ] **Step 7: Run both modules**

Run: `cd test/unit && python3 -m unittest -v test_upgrade_batch.TestAutomountedDrives test_upgrade_batch.TestFreshInstallAutomount test_automount`
Expected: OK.

- [ ] **Step 8: Commit**

```bash
git add overlay/usr/libexec/thinfloe/thinfloe-automount overlay/usr/bin/thinfloe-upgrade \
        test/unit/test_automount.py test/unit/test_upgrade_batch.py
git commit -m "fix(automount): delete a stale write hold in place, under the hold directory's lock

The stale hold was renamed aside and a fresh one linked back, so for a
moment the hold file was missing: an add of another partition of the
stick (live_hold runs outside the disk lock) could read no hold, and a
crash left .stale-<pid>. The writer now renames its hold in, and the
helper compares the inode and unlinks, under one flock of the hold
directory. Nothing is renamed aside any more.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task 2: One note for "Eject all" (D2)

**Files:**
- Modify: `overlay/usr/libexec/thinfloe/thinfloe-automount:555-566` (`commit`), `:972-1029` (`unmount_disk`), `:1032-1051` (`cmd_unmount`)
- Modify: `src/thinfloe-menu/src/removable.rs:11-13` (`EVENT_FILE` doc), `:131-158` (`Event`, `parse_event`)
- Modify: `src/thinfloe-menu/src/osd/notes.rs:19-27` (`Note`), `:46-67` (`Notes::update`), `:87-98` (`text`)
- Modify: `src/thinfloe-menu/src/osd/draw.rs:380-393` (`the_longest_stick_note_is_not_cut`)
- Test: `test/unit/test_automount.py:1072-1078` (class `Eject`), Rust tests in `removable.rs` and `osd/notes.rs`

**Interfaces:**
- Produces (helper): `commit(p, drop=frozenset(), add=None, event: Optional[tuple] = None)` where `event` is `(name, state)` or `(name, state, count)`; the line is `f"{seq}\t{name}\t{state}"` plus `f"\t{count}"` only when `count >= 2`, then `"\n"`. `unmount_disk(disk: str, p: Paths, event: bool, batch: Optional[List[str]] = None) -> Optional[int]`: with `batch` a list, a clean eject appends its event name (`", ".join` of its partitions' names, as today) to `batch` and commits no event; power-off unchanged.
- Produces (menu): `removable::Event { seq: u64, name: String, clean: bool, count: u32 }` (`count` is 1 when the field is missing, empty, not digits, 0, or does not fit u32); `osd::notes::Note::SticksRemovable(u32)`.
- Event line (contract): `seq<TAB>name<TAB>clean|unclean[<TAB>count]`.

- [ ] **Step 1: Write the failing helper tests in `Eject`**

```python
def test_eject_all(self):                      # replaces :1072-1078
    self.added("sdb", usb="1-1", label="A")
    self.added("sdc", usb="1-2", label="B")
    self.assertEqual(self.cmd("eject", "all"), 0)
    self.assertEqual(self.lines(), [])
    self.assertEqual(self.event(), ["1", "A, B", "clean", "2"])

def test_eject_all_of_one_stick_is_a_single_note(self):
    self.added("sdb", usb="1-1", label="A")
    self.assertEqual(self.cmd("eject", "all"), 0)
    self.assertEqual(self.event(), ["1", "A", "clean"])

def test_eject_all_with_one_busy_stick_is_a_single_note(self):
    self.added("sdb", usb="1-1", label="A")
    self.added("sdc", usb="1-2", label="B")
    os.environ["FAKE_UMOUNT_BUSY"] = ...      # a value that makes only A's umount busy
    self.assertEqual(self.cmd("eject", "all"), 3)
    self.assertEqual(self.event(), ["1", "B", "clean"])
```

Read `FAKES["umount"]` in the test module for how `FAKE_UMOUNT_BUSY` matches (`test_eject_busy` uses `"media"`, which matches every mount point) and pick the value that matches only `/media/.../A`; extend the fake if it cannot.

Run: `cd test/unit && python3 -m unittest -v test_automount.Eject`
Expected: `test_eject_all` FAILS (`['2', 'B', 'clean']`); the busy test FAILS or passes by order; the one-stick test passes.

- [ ] **Step 2: Implement the batch**

`cmd_unmount`: for `arg == "all" and event` pass `batch = []` to each `unmount_disk`; after the loop, one name gives `commit(p, event=(batch[0], "clean"))`, two or more `commit(p, event=(", ".join(batch), "clean", len(batch)))`, none no commit. `eject <disk>` passes no batch and is unchanged.

- [ ] **Step 3: Run the helper's tests**

Run: `cd test/unit && python3 -m unittest -v test_automount`
Expected: OK.

- [ ] **Step 4: Write the failing Rust tests**

`removable.rs`: every `Event` in `event_line` gets `count: 1`; add

```rust
#[test]
fn event_count_is_optional_and_bounded() {
    let c = |t: &str| parse_event(t).map(|e| e.count);
    assert_eq!(c("7\tA, B\tclean\t2\n"), Some(2));
    assert_eq!(c("7\tA\tclean\n"), Some(1));
    for one in ["7\tA\tclean\t", "7\tA\tclean\t0", "7\tA\tclean\tx", "7\tA\tclean\t99999999999"] {
        assert_eq!(c(one), Some(1), "{one:?}");
    }
    assert_eq!(c("7\tA\tclean\t3\tmore"), Some(3), "trailing fields ignored");
}
```

`osd/notes.rs` (the `ev` helper gets `count: 1`):

```rust
#[test]
fn a_batch_is_one_note_with_the_count() {
    let mut n = Notes::new();
    n.update(&[stick("sdb", "A"), stick("sdc", "B")], None);
    let batch = Event { seq: 1, name: "A, B".into(), clean: true, count: 2 };
    assert_eq!(n.update(&[], Some(&batch)), [Note::SticksRemovable(2)]);
    assert_eq!(text(&Note::SticksRemovable(2)), ("2 sticks can be removed".to_string(), false));
}
```

Fix the comment of `skipped_seqs_give_one_note_and_the_clean_disappearance_none` ("Eject all wrote 2, 3, 4" now reads as several ejects in a row) and keep its assertion. In `draw.rs` add `Note::SticksRemovable(16)` to the list of `the_longest_stick_note_is_not_cut`.

Run: `cd src/thinfloe-menu && CARGO_BUILD_JOBS=4 systemd-run --user --scope -q -p MemoryMax=2G -- cargo test --locked -j 4 osd:: -- --test-threads=4` and the same with filter `removable::`
Expected: does not compile (no `count`, no `SticksRemovable`).

- [ ] **Step 5: Implement `Event::count`, `Note::SticksRemovable` and its text**

`Notes::update`: a clean event with `count >= 2` gives `Note::SticksRemovable(count)`, else `Note::Removable(name)` as today. `text`: `format!("{n} sticks can be removed")`, not a warning. Update the `EVENT_FILE` and `Event` doc comments to the new line.

- [ ] **Step 6: Run the menu's checks**

Run: `make test-menu`
Expected: fmt and clippy clean, all tests pass.

- [ ] **Step 7: Commit**

```bash
git add overlay/usr/libexec/thinfloe/thinfloe-automount test/unit/test_automount.py \
        src/thinfloe-menu/src/removable.rs src/thinfloe-menu/src/osd/notes.rs src/thinfloe-menu/src/osd/draw.rs
git commit -m "fix(automount): eject all writes one event for the batch, the OSD says how many

eject all replaced the event line once per disk, so the OSD showed only
the last stick's note. The batch now writes one line, with a fourth
field, the count, when it ejected two sticks or more (a single event
keeps its three fields); the OSD shows \"<n> sticks can be removed\".

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task 3: A mount number in the contract file, so a swapped card is new (D3)

**Files:**
- Modify: `overlay/usr/libexec/thinfloe/thinfloe-automount:15-19` (docstring), `:121-130` (`Line`), `:352-372` (`parse_lines`, `format_lines`), `:555-566` (`commit`)
- Modify: `src/thinfloe-menu/src/removable.rs:1-7` (module doc), `:28-36` (`Stick`), `:76-113` (`Line`, `parse_line`, `parse`)
- Modify: `src/thinfloe-menu/src/osd/notes.rs:71-75` (`identity`), test helper `:242-250`
- Modify (new field only): `src/thinfloe-menu/src/ui.rs:2820-2826` and `:7742-7748`, `src/thinfloe-menu/src/menu.rs:5231-5239`
- Test: `test/unit/test_automount.py` classes `Lines` (`:287-314`) and `Add`; Rust tests in `removable.rs` and `osd/notes.rs`

**Interfaces:**
- Consumes: `commit` as changed in Task 2.
- Produces (helper): `Line.mount: int = 0` (last dataclass field); `LINE_FIELDS` stays 8 (the ninth is optional); `next_mount(p: Paths) -> int`, called only under `files_lock`: reads `os.path.join(state_dir(p), "mount-seq")` (digits, else 0), `atomic_write`s it plus one and returns that; `commit` sets `add.mount = next_mount(p)` before it writes.
- Produces (menu): `Stick.mounts: Vec<u64>`, parallel to `names` (0 for a line without a valid ninth field); `identity(s)` = disk, names and mounts.
- Contract line: `device<TAB>disk<TAB>name<TAB>mountpoint<TAB>fstype<TAB>size_bytes<TAB>model<TAB>kind<TAB>mount`.

- [ ] **Step 1: Write the failing helper tests**

`Lines.test_lines_round_trip_and_tolerance`: give its two `mod.Line(...)` `mount=1` and `mount=2` (the round trip keeps them). Add to `Lines`:

```python
def test_mount_field_is_optional(self):
    eight = "\t".join(["/dev/sdb1", "sdb", "N", "/m/N", "vfat", "5", "M", "stick"])
    self.assertEqual(mod.parse_lines(eight + "\n")[0].mount, 0)
    self.assertEqual(mod.parse_lines(eight + "\tx\n")[0].mount, 0)
    self.assertEqual(mod.parse_lines(eight + "\t7\textra\n")[0].mount, 7)
```

Add to `Add`:

```python
def test_each_mount_gets_a_new_number(self):
    # A card swapped for one of the same name is another mount.
    self.stick("sdb", label="CARD", kind_card=True)
    self.assertEqual(self.cmd("add", "sdb1"), 0)
    first = self.lines()[0].mount
    # take the card out and put the same-named one in, the way
    # Remove.test_remove_card_pulled_from_reader does, then add again
    self.assertEqual(self.cmd("add", "sdb1"), 0)
    self.assertEqual(self.lines()[0].name, "CARD")
    self.assertGreater(self.lines()[0].mount, first)
```

Run: `cd test/unit && python3 -m unittest -v test_automount.Lines test_automount.Add`
Expected: FAIL/ERROR (`Line` has no `mount`).

- [ ] **Step 2: Implement `Line.mount`, `next_mount` and the ninth field**

`parse_lines` takes `f[8]` when present and all digits, else 0; `format_lines` always writes it. Update the docstring's contract line.

- [ ] **Step 3: Run the helper's and the writer's tests**

Run: `cd test/unit && python3 -m unittest -v test_automount test_upgrade_batch.TestAutomountedDrives`
Expected: OK (`read_removable_rows`, `thinfloe-upgrade:819-830`, already ignores fields after the eighth).

- [ ] **Step 4: Write the failing Rust tests**

`removable.rs`:

```rust
#[test]
fn mount_field_is_optional_and_garbage_is_zero() {
    let s = parse("/dev/sdb1\tsdb\tA\t/m\tvfat\t5\tM\tstick\t7\n/dev/sdb2\tsdb\tB\t/m2\tvfat\t5\tM\tstick\n");
    assert_eq!(s[0].mounts, [7, 0]);
    assert_eq!(parse("/dev/sdb1\tsdb\tA\t/m\tvfat\t5\tM\tstick\tx\n")[0].mounts, [0]);
}
```

Keep `unknown_trailing_fields_are_ignored`. `osd/notes.rs`: the `stick` helper gets `mounts: vec![0]`; add `stick_m(disk: &str, name: &str, mount: u64) -> Stick` and

```rust
#[test]
fn a_card_swapped_for_one_of_the_same_name_is_new() {
    let mut n = Notes::new();
    n.update(&[stick_m("sdb", "CARD", 1)], None);
    assert!(n.update(&[stick_m("sdb", "CARD", 1)], None).is_empty());
    assert_eq!(n.update(&[stick_m("sdb", "CARD", 2)], None), [Note::Connected("CARD".into())]);
}
```

Run: `cd src/thinfloe-menu && CARGO_BUILD_JOBS=4 systemd-run --user --scope -q -p MemoryMax=2G -- cargo test --locked -j 4 removable:: -- --test-threads=4`
Expected: does not compile (no `mounts`).

- [ ] **Step 5: Implement `Stick.mounts` and the identity**

`parse_line` reads `f.get(8)` (all ASCII digits and parses as u64, else 0); `parse` pushes it next to the name. `identity` appends the mounts to disk and names. Add `mounts: vec![0]` to the three constructors in `ui.rs` and `menu.rs` listed above. Update the module doc's line format.

- [ ] **Step 6: Run the menu's checks**

Run: `make test-menu`
Expected: fmt and clippy clean, all tests pass.

- [ ] **Step 7: Commit**

```bash
git add overlay/usr/libexec/thinfloe/thinfloe-automount test/unit/test_automount.py \
        src/thinfloe-menu/src/removable.rs src/thinfloe-menu/src/osd/notes.rs \
        src/thinfloe-menu/src/ui.rs src/thinfloe-menu/src/menu.rs
git commit -m "fix(automount): a mount number in the contract file, so a swapped card is new

The OSD told sticks apart by disk and partition names: a card swapped
for one with the same name, with no read of the list in between, looked
like the same card and got no \"connected\" note. Each mount now gets a
number (per boot, automount/mount-seq), appended as a ninth contract
field, and the OSD's identity includes it. Older lines read as 0.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task 4: CHANGES.md, docs/notes/automount.md, CLAUDE.md (D4, and the record of D1 to D3)

**Files:**
- Modify: `CHANGES.md` (`## [Unreleased]` → `### Fixed`)
- Modify: `docs/notes/automount.md`: "Contract and event files", "Write hold", "Locks", "Tests"
- Modify: `CLAUDE.md` (Rules that are easy to break → Automount)

- [ ] **Step 1: CHANGES.md, under Fixed**

```markdown
- After Eject all with several sticks plugged in, the note at the bottom of the screen named only one of them. It now says how many can be removed, for example "2 sticks can be removed".
- A card swapped in a card reader for another card with the same name gave no "… connected" note. It now does.
```

D1 gets no entry: nothing a user or administrator sees changes; the commit message carries it. D4 changes no behaviour.

- [ ] **Step 2: docs/notes/automount.md**

- Contract line: append `<TAB>mount`, and one sentence: a number per mount, counted per boot in `/run/thinfloe/automount/mount-seq`, so a card swapped for one of the same name is a new stick to the OSD; 0 in a line of an older helper.
- Event line: `seq<TAB>name<TAB>clean|unclean[<TAB>count]`. Replace "`eject all` replaces the line once per disk, so a reader sees only the last." with "`eject all` writes one line for the batch: the names joined with ", " and, for two sticks or more, their count."
- Write hold: replace the sentences from "a stale hold is removed, but only the file that was read" to "always stays." with: the writer renames its hold into place, and the helper deletes a stale one (the inode it read, compared again) only while it holds an flock on the hold directory; nothing is renamed aside, so the file is never missing.
- Locks: add "The hold directory's flock is taken by `thinfloe-upgrade` around the rename of its hold and by the helper around the delete of a stale one, waited for 5 s at most (`HOLD_LOCK_WAIT`; then the writer writes unlocked and the helper keeps the stale hold). Nothing else is taken while it is held."
- Tests, the `make qemu-smoke-two-sticks` bullet: add "Both sticks stay on virtio (ruling 2026-11-26): the test checks the choice of the stick by its ID, which does not depend on the bus; `make qemu-smoke-automount` covers USB."

- [ ] **Step 3: CLAUDE.md, Automount rule**

Append `<TAB>mount` after `kind` in the contract format, and after "Every write of `thinfloe-upgrade` holds the stick and releases it before the first byte" add "; its hold file is renamed in, and a stale one deleted, only under an flock of the hold directory, never renamed aside".

- [ ] **Step 4: No stale wording left**

Run: `grep -n "stale-\|renamed aside\|once per disk" docs/notes/automount.md overlay/usr/libexec/thinfloe/thinfloe-automount`
Expected: only the new "never renamed aside" / "nothing is renamed aside" wording, no description of the old mechanism.

- [ ] **Step 5: Commit**

```bash
git add CHANGES.md docs/notes/automount.md CLAUDE.md
git commit -m "docs(automount): batch event, mount number, hold lock, two-sticks ruling

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

### Task 5: Gates

The branch changes the root helper, `thinfloe-upgrade`, both contract files and the OSD in the menu crate, so CLAUDE.md "Workflow" asks for the full set before the branch joins the round (in this order, each green before the next).

- [ ] **Step 1:** `make test-unit`. Expected: exit 0 (all Python modules OK, menu and share `cargo test` pass).
- [ ] **Step 2:** `make image`, then `make test`. Expected: exit 0, 0 FAIL (`test-automount-image.sh` among them).
- [ ] **Step 3:** `make test-runtime` (the OSD is a menu mode). Expected: exit 0.
- [ ] **Step 4: the smoke tests the change touches**, one after another, on the image of Step 2:
  - `make qemu-smoke`: helper and OSD start on every boot;
  - `make qemu-smoke-automount`: contract, events, eject, eject all; `event_is` pins the three-field single event;
  - `make qemu-smoke-write`: hold, release and remount around a write;
  - `make qemu-smoke-two-sticks`: the helper refuses the second ThinFloe stick; the D4 ruling keeps this test as it is.
  Expected: each ends with 0 failed checks. `PREBUILT=1` may be added only when the source key next to `artifacts/rootfs/` is this checkout's (the link is shared between checkouts).
- [ ] **Step 5:** Put the results (ok and FAIL counts per target) in the round PR body under this branch's heading. No commit.

---

## Self-review

- Spec coverage: D1 → Task 1 (deviations: a lock instead of a rename back; no `.stale-*` cleanup); D2 → Task 2; D3 → Task 3 (deviation: the number is a contract field, the event file had one already); D4 → Task 4 Step 2 and Task 5 Step 4. Each item has a CHANGES line or a stated reason for none, and a line in automount.md.
- Types: `commit`'s event tuple from Task 2 is used unchanged in Task 3; `Event.count: u32` and `Note::SticksRemovable(u32)` match; `Stick.mounts: Vec<u64>` is parallel to `names` wherever a `Stick` is built.
- Review Focus: lines 1 to 5 each have a named test in Tasks 1 to 3.
