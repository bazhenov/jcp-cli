# Installation

At the moment installation requires rust toolchain:

```console
$ git clone https://github.com/JetBrains/jcp-cli
$ cd jcp-cli
$ cargo install --path=.
```

# Configuring

1. create `.env` file with `AI_PLATFORM_TOKEN` defined to OAuth2 Development Token from https://platform.stgn.jetbrains.ai. or use IDE configuration to pass env-variable
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
      "AI_PLATFORM_TOKEN": "...",
    },
  },
},
```
