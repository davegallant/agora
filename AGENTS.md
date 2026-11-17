# agora

## Git

Work on `main`: commit approved changes straight to it and push. This personal
repo uses no feature branches or PRs. A push to `main` runs the `ci` workflow,
and changes under `src/` or `demo/` also run `demo`, which re-records the README
GIF and opens a PR with it.

## Checking a change

`nix develop` provides the toolchain. Run `cargo test` and
`cargo clippy --all-targets`, then `demo/try.sh` to drive the TUI against
made-up sessions in a throwaway `$HOME`.
