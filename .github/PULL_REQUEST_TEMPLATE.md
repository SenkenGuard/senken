<!--
The title of this pull request becomes the commit message when it is squashed,
and the release notes are generated from those messages. Write it as a
conventional commit, e.g. `feat(charts): draw fib retracements`.
See CONTRIBUTING.md for the types.
-->

## What this changes

## Why

## Verification

<!-- What you actually ran. Delete lines that do not apply. -->

- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- [ ] `cargo test --locked --workspace`
- [ ] `bun run --filter web check`
- [ ] A test covers the new behaviour, and I watched it fail before the fix

## Anything the diff cannot say

<!-- A venue quirk, an ordering constraint, an alternative you rejected and
     why, a limit that will bite later. Leave blank if there is none. -->
