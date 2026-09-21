# OpenPGP security watch

Snail pins `pgp` exactly because its security history is not completely represented in RustSec.
The maintainer must watch the upstream [rpgp security advisories](https://github.com/rpgp/rpgp/security/advisories)
and review that page before every version change. GitHub's **Watch → Custom → Security alerts** is
the required repository subscription; this manual account setting cannot be committed to source.

Upgrade checklist:

1. Read every advisory published since the last pinned release, including private/advisory-page
   entries that do not appear in RustSec.
2. Re-run `cargo audit`, `cargo deny check advisories`, the hostile parser tests, and the fuzz target.
3. Confirm `pgp` still has neither its default feature nor `asm` enabled and that no OpenPGP edge
   pulls `cc`.
4. Reassess each documented advisory ignore in both `deny.toml` and `.cargo/audit.toml`.
5. Keep the decompressed-size and nesting limits even if upstream raises or removes its own limit.

WKD is implemented against draft-22 (2026-07-22). It remains an Internet-Draft and a useful
discovery convention, not an authoritative trust signal.
