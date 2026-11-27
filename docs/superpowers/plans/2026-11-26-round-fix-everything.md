# Round 2026-11-26b: fix every outstanding issue

Owner order 2026-11-26 14:20: "take care of all the outstanding issues". This
document is the spec of the round. It lists every open item, the branch that
owns it, and the decision for it. Each branch has its own plan in
`docs/superpowers/plans/2026-11-26-<branch>.md`.

Rule for every item: fix it. Where a fix is wrong or out of proportion, the
plan says so with a reason, and the item gets a line under "Known limits" in
the matching `docs/notes/*.md`, so no item stays only in a PR body.

Round branch: `round/2026-11-26b`. Branches are rebased onto it in turn
(CLAUDE.md, Workflow). Each branch: gates, one whole-branch review, one fix
wave, one scoped re-review.

## Branch A: fix/floedrop-hardware

The bugs from the hardware test on thinfloe-2b3967 (4.13.0, TP-Link
RTL8812BU stick) and the card-choice follow-ups of PR #62.

| # | Item | Decision |
|---|------|----------|
| A1 | Band, state file and log say "channel 6" while the radio runs on 44 (probe channel taken before radio.toml's country applies, radio.rs mode_channel). | Report the channel of the link that came up, read after the country is applied (owlyd Link property or `iw dev` of the station). The ready line changes its value only, not its format. |
| A2 | No sign of a running transfer; no log line for a received, declined or failed delivery. | New per-receiver state `receiving` in the state file's receiver lines (the field already exists; readers of older menus show an unknown state as idle). The band shows "Receiving <file> from <sender>" (or "<n> files"); the daemon logs one line per incoming offer, per accept/decline and per finished or failed delivery, with sender, file count and bytes. Progress in bytes only if Owly exposes it; else start and end. |
| A3 | Zone UTC (no zone from menu or DHCP) gives no country, regdom 00, channel 6, slow start, slow transfer; nothing said. | Fallback country: when the zone gives none, take it from the keyboard layout's country (`[keyboard]` layout code, e.g. `ch` -> CH, `de` -> DE; a table for layouts that are not country codes, `us` -> none, `gb` -> GB). If neither gives one: start with regdom 00 and show a warning in the band: "No country set: set the time zone in Settings → Time for faster FloeDrop." |
| A4 | "Ready" while the stick cannot send (rtw88 firmware wedged; owlyd STATS "radio full"/"gave up" climb); later starts fail "Couldn't bring owlyibss0 up: Device or resource busy (os error 16)". Only a replug helped. | Detect it: a watch on owlyd's counters (the source the plan finds: owlyd STATS over D-Bus or the journal) with a threshold the plan fixes and a test pins; EBUSY at radio start counts too. Then the state becomes `error` with "The Wi-Fi stick <card> stopped sending. Unplug it and plug it in again." and the radio is released. |
| A5 | After FloeDrop off/on the iPhone listed the receiver twice and sent the files to both (" (2)" copies). | Find whether the old mDNS/AWDL service is withdrawn on stop; withdraw it (or reuse the same instance name) so a restart shows one receiver. A test pins the withdraw on stop. |
| A6 | A USB stick that Owly's probe refuses beats a usable internal card; no fallback. | The helper tries the cards in order of preference and takes the first that the probe accepts. |
| A7 | The 3160 error says "plug in a USB Wi-Fi stick" also when one is plugged in but used for the network. | Name the case: "… the USB stick <card> is used for the network." when such a card exists. |
| A8 | "FloeDrop cannot start on …" (daemon.rs ~491) names awdl0. | Name the station card and driver, as the ready line does. |
| A9 | Intel AC 7260 (8086:08b1/08b2), same firmware family, untested, not on the skip list. | Add it to the skip list (same firmware, same crash risk); a USB card or a newer Intel card is used instead. Note in floedrop.md that it is skipped by family, untested. |
| A10 | docs/notes/floedrop.md says qemu-smoke shows the band's error text; nothing checks it. | Correct the note to what is tested; no new smoke check. |
| A11 | Literal backslash-backticks in daemon.rs doc comments. | Fix them. |

Every FloeDrop toggle saves the home: accepted, no change.

## Branch B: fix/floedrop-followups

The follow-up list of PR #57 (FloeDrop). The plan takes every bullet of that
list (`gh pr view 57 --json body`, "Follow-ups") and decides fix or known
limit per bullet, under these rulings:

- One folder per phone: check on hardware is open; make the folder key
  robust (match by the sender's stable id if Owly gives one), else known limit.
- awdl/mDNS key order: make the choice deterministic (prefer the awdl key).
- Prune race: fix (prune under the same lock as insert, or re-check).
- OSD note width at scale > 1: fix with the scale file, for the note panel;
  add a note check to test-runtime only if cheap.
- Send needs twice the batch size: known limit, but the band or OSD says why a
  batch failed ("not enough room in FloeDrop for <n> MB").
- Card guard only at start: known limit (documented), FloeDrop says it.
- convert.rs drops `share`: fix.
- Notes/route filter wording: fix the note.
- Names, card status, inbox, outbox, radio, daemon, menu, OSD bullets: fix
  each one that is a defect or a missing test; code-tidy bullets (two impl
  blocks, `unreachable!`, OSD_ICON centring, file name deskflow_sup.rs)
  are done when cheap, else dropped with a line in the plan.
- "A missing tmpfs is only logged while the state says ready (recheck when
  the automount branch lands)": recheck now; state `error` if FloeDrop's
  tmpfs is not mounted.

Branch B is rebased after A (both touch src/thinfloe-share/).

## Branch C: fix/tile-followups

PR #62 tile-notices list.

| # | Item | Decision |
|---|------|----------|
| C1 | Hovered mark's highlight covers the end of a long comment on a short tile. | Reserve the mark's width in the comment's wrap width. |
| C2 | Greyed tile: mark highlight and focus ring not faded. | Fade them like the tile. |
| C3 | A test recomputes the text column by hand. | Take it from the drawing code's layout function. |
| C4 | Tab stops on the mark as an extra step. | Keep it (keyboard users need to reach Packages) and document it in menu.md. |
| C5 | test-session-packages.sh does not check 70-eject or square-terminal. | Add both checks. |
| C6 | No pointer cursor anywhere (from the handoff). | Show the pointing-hand cursor over clickable tiles, marks and buttons, if winit's cursor works on labwc; else known limit. |

## Branch D: fix/automount-followups

PR #58 list.

| # | Item | Decision |
|---|------|----------|
| D1 | Hold put-back: hold file missing for a moment; a crash then leaves `.stale-<pid>`. | Put back with an atomic rename so the file is never missing; the cleanup removes old `.stale-*`. |
| D2 | "Eject all" shows only the last stick's note. | One note for the batch: "<n> sticks can be removed." |
| D3 | Two cards of the same name swapped in one reader with no list read between: no "connected" note. | Event file carries a sequence number (contract field appended at the end; readers ignore unknown trailing fields). |
| D4 | Two-sticks smoke keeps stick A on virtio; "needs a ruling". | Ruling: keep it. The two-sticks test checks the stick choice by ID, which does not depend on the bus; automount covers USB. Note it in automount.md. |

## Branch E: fix/test-backlog

The defect backlog of the handoff (16 items).

| # | Item | Decision |
|---|------|----------|
| E1 | flock passes its fd to podman; a stopped build leaves the lock stuck. | Close the fd for the child (`flock -o`) in the documented commands and any script that wraps a build. |
| E2 | `test-dhcp-timezone.sh` mktemp falls back to /tmp. | Use the test's own temp dir like the other tests. |
| E3 | Two #26 tests vacuous under load. | Make them assert what they claim (find them by the #26 reference). |
| E4 | ~10 stale `buildah ps` working containers. | List them for the owner; removal only after owner OK (not in the branch). |
| E5 | `run-qemu-smoke.sh` wrong timeout message while waiting for `SMOKE_WAIT_FOR`. | Fix the message. |
| E6 | `network_apply` loses a `[network]` key changed while the tool runs. | Merge on write: re-read the store and apply only the job's keys. |
| E7 | `app.rs` keeps only the last `run_ui` pass's event. | Keep all events of a frame (queue). |
| E8 | `tests/render_icon.rs` timing-sensitive under load. | Widen or remove the timing dependence. |
| E9 | `osd::socket::tests::send_without_a_server_is_no_server` failed 1/40. | Find the race and fix it. |
| E10 | conntile RTF_GATEWAY 0 warns `none`. | Accepted earlier; no change. |
| E11 | Unexplained `make test-unit` exit 2 (1/8 local). | Reproduce in a loop with logs; fix the cause or log it in the plan as not reproduced after N runs. |
| E12 | `test_upgrade_batch` (94 s) bounds parallel Python time. | Split or speed it up. |
| E13 | `TF_RUNTIME_MENU` host-built menu panics in the runtime container ("failed to generate random data"). | Find the cause (getrandom in the container) and fix or document the option. |
| E14 | Deskflow clipboard-larger-than-TLS-default test flaked under load. | Fix the race. |
| E15 | Old advice `-A clippy::chunks-exact-to-as-chunks` breaks host clippy. | Remove that advice wherever it is written; `make test-menu` is the way. |
| E16 | test-save-home.sh test 17 fails in a path containing "mount". | Fixed in #58; verify and drop. |

## Track F: docs/site-4.13 (own PR, not in the round)

New `site/img/menu.webp` and `menu.png` from a VM with a plugged virtual
stick (Eject tile visible), per site/README.md; the page describes 4.13.0.
