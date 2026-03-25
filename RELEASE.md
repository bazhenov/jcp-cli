## Release

1. update version in `Cargo.toml`
2. create a tag
3. push changes to remote
4. push main branch and release tag to `bazhenov/jcp-cli` (temporary workaround while figuring out PAT for jetbrains/jcp-cli).
5. From this point `ci.yaml` GHA workflow will kickin and it will:
   - build release distribution on all platforms
   - create GitHub Release
   - update Homebrew Folmulae for macOS
