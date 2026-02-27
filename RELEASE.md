## Release

1. `cargo release (patch|minor|major) --execute`. It will:
  - update `Cargo.toml`
  - create tag
  - push changes to remote
2. push main branch and release tag to `bazhenov/jcp-cli` (temporary workaround while figuring out PAT for jetbrains/jcp-cli).
3. From this point `ci.yaml` GHA workflow will kickin and it will:
  - build release distribution on all platforms
  - create GitHub Release
  - update Homebrew Folmulae for macOS
