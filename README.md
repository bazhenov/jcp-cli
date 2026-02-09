# Installation

## Homebrew (macOS)

```console
$ brew install bazhenov/tap/jcp
```

## Building from sources

At the moment installation requires rust toolchain:

```console
$ git clone https://github.com/JetBrains/jcp-cli
$ cd jcp-cli
$ cargo install --path=.
```

# Configuring

1. create `.env` file in the project directory or any parent of with `AI_PLATFORM_TOKEN` in it defined to OAuth2 Development Token from the [Staging](https://platform.stgn.jetbrains.ai) or use IDE configuration to pass env-variable
2. do `jcp login`
3. configure your IDE with `jcp acp` as an ACP agent.

## Zed

```json
"agent_servers": {
  "JCP": {
    "type": "custom",
    "command": "jcp",
    "args": ["acp"],
    "env": {
      // You can put AI_PLATFORM_TOKEN here or in .env file
      "AI_PLATFORM_TOKEN": "...",
    }
  }
}
```

# Developing

When running locally we're using file-backend for a storing secrets instead of platform keychain, because macOS keeps asking for a password on each recompile which significantly slows down the development process.
