# Changelog fragments

Each pull request adds its changelog entry as a file in `new/`, `changed/` or
`fixed/`. The release pull request moves the entries into `CHANGES.md`, under
`### New`, `### Changed` and `### Fixed`, and deletes the files.

A file holds one or more entries. An entry is a line that starts with `- `,
followed by lines indented by two spaces. A heading or any other text is
refused.

A file name is the UTC time to the minute, 4 random hex characters and a
topic, for example `new/20261126-0921-a3f9-time-band.md`. These commands print
the first two parts:

```sh
date -u +%Y%m%d-%H%M
openssl rand -hex 2
```

Within a section, the entries are released in the order of the file names,
so the oldest file comes first.

If a pull request edits a fragment that a release has deleted in the meantime,
the rebase stops on a modify/delete conflict. Drop the file with `git rm`, since
keeping it releases the entry twice.
