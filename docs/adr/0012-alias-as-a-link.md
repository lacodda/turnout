# ADR 0012: The short alias is a link, not a second binary

- Status: accepted
- Date: 2026-08-27

## Context

turnout installs under two names: `turnout` and the short `tn`. Until 0.10.3 the Windows installer produced the second one with `Copy-Item` - symlinks on Windows need elevation or developer mode, which an installer has no business demanding, so a copy looked like the only option. `install.sh` had always used a symlink.

Two problems followed from the copy, both found on the owner's machine after installing 0.10.3:

- **The alias went stale.** `turnout self-update --yes` updated `turnout.exe` to 0.10.3 and left `tn.exe` at 0.10.2 beside it. self-update replaces `std::env::current_exe()`; the copy is a different file and nothing touched it. The trap is that `tn --version` honestly printed `turnout 0.10.2`, which reads as an inexplicable downgrade rather than a stale alias.
- **It cost a whole binary.** Two 13.7 MB files where one would do, for the same code twice over. Measured on the same machine: a copy costs 13.8 MB of disk, a link costs nothing.

A third artefact was visible in the same directory: `tn.exe.old` from 13.08, left by an update that had run under the alias name. `sweep_backup` only looked beside `current_exe`, so nothing was ever going to collect it.

kasl solved the same problem differently in v1.4.0: `ka` is declared as a second `[[bin]]`, packaged into the release archive, and an update replaces both. That works, but it doubles the size of every download and every install to ship the same bytes twice - which is the cost this ADR rejects.

## Decision

**The alias is a link to the binary, created at install time; never a copy, and never a second packaged binary.**

- **Unix:** a symlink, relative (`ln -sf turnout`), so the link survives the install directory being moved.
- **Windows:** a **hard** link. It needs no elevation, unlike a symlink, as long as both names are on one volume - and they are, since the alias lands in the install directory beside the binary. Verified without elevation on the owner's machine before committing to it. A copy remains the fallback for a filesystem that has no hard links, so the alias still exists there; it is just the degraded case, not the normal one.

**`self-update` re-points the alias after the swap.** Replacing the running binary renames the outgoing file aside, which breaks the link - without a relink the alias would follow the old file and reproduce exactly the reported bug. The relink is unconditional rather than conditional on the alias having gone stale: telling those apart means asking whether two paths are the same file, which Windows has no stable std API for (`file_index` is nightly-only), and relinking is a cheap directory operation either way.

The swap and the relink live in one function (`install_binary`) so that neither can be done without the other. That is not tidiness - the field report *was* the half-update.

An install with no alias does not grow one from an update: `TURNOUT_NO_ALIAS` is the user's choice, and an update is no place to overrule it.

**This is the line's rule, not just turnout's**: any product shipping a short alias links it. kasl's second-`[[bin]]` approach is to be brought over to this one.

## Consequences

- One binary on disk answers to both names. An install is half the size it was, and the two names cannot drift apart - there is nothing to keep in sync.
- The stale-alias failure is gone at the root rather than patched: it was possible only because there were two files.
- The release archive stays one binary. Two guards in `tests/release_consistency.rs` hold that: one fails if an installer copies instead of linking, one fails if a second `[[bin]]` or a packaged `tn` appears.
- `sweep_backup` now collects `tn.old`/`tn.exe.old` as well as its own, clearing the leftovers of installs made before this change.
- A failed relink is reported rather than swallowed, because a stale alias keeps answering under its own name - silence is what made the original bug hard to read.
- Hard links are an NTFS/ReFS feature. On an exotic filesystem the installer falls back to a copy and says so, and that install keeps the old behaviour: the alias is refreshed by re-running the installer, not by an update.
