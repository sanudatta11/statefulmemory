# auth.md — statefulmemory

`auth.md` describes how agents authenticate with statefulmemory. This document
covers both the public documentation origin (`statefulmemory.dev`) and the
self-hosted memory daemon that the CLI / MCP server talk to.

## Audience

AI coding agents (Claude Code, Cursor, OpenCode, Codex, Gemini CLI, and peers)
that install the `statefulmemory` CLI, call its MCP server, or talk to a
self-hosted team daemon.

## Public origin (`statefulmemory.dev`)

The documentation site is **public and unauthenticated**. It serves only static
documents: HTML docs, `llms.txt`, `llms-full.txt`, `/.well-known/api-catalog`,
`/.well-known/agent-skills/index.json`, and this `auth.md`. There is no account
system, no registration endpoint, and no credential to obtain for reading the
site. No authentication is required for agent discovery.

- Registration: none required.
- Credentials: none.
- Method: anonymous, read-only.

## Local daemon (single user)

The default deployment is single-user and local. The CLI auto-spawns a daemon
that listens on a Unix domain socket (`~/.statefulmemory/daemon.sock`, mode
`0600`). Access is controlled by filesystem permissions — no token is needed on
the same machine and same user.

```bash
statefulmemory install        # wire the agent (MCP + skill + hooks)
statefulmemory daemon start   # auto-spawned on first use
```

## Team daemon (shared, TCP + TLS)

For a shared team daemon, statefulmemory uses TLS with bearer tokens and
per-project grants. There is **no OAuth/OIDC provider**: tokens are issued and
managed by the `statefulmemory team` verbs.

Provision and issue credentials:

```bash
statefulmemory team init-ca                 # create the local CA
statefulmemory team token-create --name ci  # mint a bearer token
statefulmemory team grant <project> <token> # scope a token to a project
```

Credential use: send the token as an HTTP `Authorization: Bearer <token>`
header (gRPC metadata `authorization: Bearer <token>`) on every TCP request.
Tokens carry no refresh flow; rotate by minting a new token and revoking the old
one. See [self-hosting](https://statefulmemory.dev/docs/self-hosting/) for the
full runner.

## Method summary

| Deployment | Registration | Credential | Method |
|---|---|---|---|
| Public docs (`statefulmemory.dev`) | none | none | anonymous |
| Local daemon | none | none (socket `0600`) | local Unix socket |
| Team daemon | `team token-create` | bearer token | `Authorization: Bearer` over TCP+TLS |

## Notes

- statefulmemory does **not** publish OAuth authorization-server or OpenID
  Connect metadata, because no hosted OAuth/OIDC authorization server exists.
  Any document claiming otherwise would be inaccurate.
- Do not probe for a registration endpoint; there is none. Discovery documents
  on this origin are the source of truth.
