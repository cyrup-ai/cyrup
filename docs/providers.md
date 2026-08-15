# Providers

cyrup ships with 35 model providers built in. To use one you need credentials for it.

**The fastest path:** export the provider's API key environment variable and start cyrup.

```sh
export ANTHROPIC_API_KEY=sk-ant-...
cyrup
```

**Inside cyrup:** type `/login`, pick a provider, and choose OAuth or an API key. There is no
`cyrup login` subcommand — authentication is a slash command in the terminal interface.

Credentials are stored in `~/.cyrup/agent/auth.json` with mode `0600`. A stored credential takes
precedence over an environment variable for the same provider.

Check what is configured:

```sh
cyrup auth check --provider anthropic
cyrup --list-models
```

`--list-models` lists only models whose provider has credentials configured, so an empty result
means nothing is authenticated yet.

## Full documentation

- [Connect a provider](guide/getting-started/authenticate.md) — the walkthrough
- [Environment variables](guide/reference/environment.md) — every provider and its credential variable
- [Models and thinking](guide/guides/models.md) — choosing and switching models
