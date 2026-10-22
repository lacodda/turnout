# ADR 0029: A Windows server is proven on a real one

- Status: accepted
- Date: 2026-10-22

## Context

Since v0.24 every command turnout sends to a Windows server runs through a real `cmd.exe` in the unit tests, behind an SSH server written for the tests. That caught the shell's own rules. It could not catch the rest of a Windows server: sshd's sessions and environment, its SFTP server, user profiles, which authorized-keys file it reads for whom, and which ACL it accepts on that file. No Windows stand was at hand for a live run either, so every release shipped the Windows path on the strength of the shell alone.

## Decision

**Every push and every release tag runs turnout against a real Windows sshd.** The `windows-server` workflow starts OpenSSH Server on a GitHub Actions Windows runner, answering with `cmd.exe` as it does out of the box, with two administrator accounts and one ordinary account (`tools/ci/windows-sshd.ps1`). `tests/windows_server.rs` then lives a stand's whole life through - deploys as one archive and file by file, `--diff`, backup, restore, prune, a deploy rolled back by its check, `rollback --to`, `--clear`, `exec`, `doctor --server` - and `key setup` for both administrators in turn and for the ordinary account.

**The server is the test machine**, so every result is read off the disk itself - the files that landed, the backups, the shared key file - rather than from what turnout says about them.

**The tests are `#[ignore]`d and fail without the server.** A plain `cargo test` has no sshd; the workflow runs them with `--ignored`. Run without the variables the script exports, a test fails and names the script - a proof that quietly skipped reads exactly like one that passed.

**Release and Publish both wait for it.** The workflow is reusable and called from CI, Release and Publish. Gating one tag workflow and not the other would be worse than gating neither: the npm package downloads the binary from the GitHub release, so a blocked release beside a published npm package is a broken install.

## Consequences

- The first runs found four defects no earlier test could: the shared `administrators_authorized_keys` was restricted to the first administrator who set up a key, locking out every other; an ordinary account's key file was granted to a name Windows OpenSSH builds as `WORKGROUP\name`; `--json` output carried a progress line in front of it; and the learned shell was invisible and outlived a change of address.
- A Windows runner image that changes (a new OpenSSH, a new default `sshd_config`) can turn a release red without a change in turnout. That is the point: it would have broken real stands the same way.
- A physical Windows stand is no longer what a release waits for.
