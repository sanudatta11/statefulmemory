# statefulmemory.dev

Public docs site for [statefulmemory](https://github.com/sanudatta11/statefulmemory), built with
[Astro Starlight](https://starlight.astro.build/) and deployed to GitHub Pages.

Canonical URL: **https://statefulmemory.dev**

## Local development

```bash
cd website
npm ci
npm run dev
```

```bash
npm run build    # output → dist/
npm run preview
```

## Go-live checklist (domain + Search Console)

Do these once before (or right after) the first successful `Pages` deploy on `main`:

1. **Enable GitHub Pages (required)**
   - Repo → **Settings → Pages**
   - Build and deployment → Source: **GitHub Actions**
   - Without this, `actions/deploy-pages` fails with `HttpError: Not Found`
2. **Custom domain**
   - Custom domain: `statefulmemory.dev`
   - Enable **Enforce HTTPS**
3. **DNS at your registrar** (confirm IPs in Pages settings if they change)
   - Apex `statefulmemory.dev`: A records to GitHub Pages IPs
   - `www.statefulmemory.dev`: CNAME → `sanudatta11.github.io`
   - Prefer apex as canonical; redirect www → apex when Pages offers it
4. **Google Search Console**
   - Add a **Domain** property for `statefulmemory.dev` (or URL-prefix `https://statefulmemory.dev/`)
   - Verify ownership with a **DNS TXT** record at the registrar
   - Submit sitemap: `https://statefulmemory.dev/sitemap-index.xml`
5. Confirm crawlability
   - `https://statefulmemory.dev/robots.txt` lists the sitemap and allows AI search bots
   - `https://statefulmemory.dev/sitemap-index.xml` returns 200 after deploy
   - `sitemap-0.xml` lists every docs page (home + getting-started, install,
     agents, commands, config, locomo, troubleshooting); `/404` is excluded
6. Confirm LLM-ready surfaces
   - `https://statefulmemory.dev/llms.txt` (curated index)
   - `https://statefulmemory.dev/llms-full.txt` (plain-text product summary)
   - Repo root `AGENTS.md` / `CLAUDE.md` for coding agents cloning the source

`public/CNAME` already contains `statefulmemory.dev` for the Pages custom domain.

After enabling Pages, re-run the failed **Pages** workflow on `main` (Actions → Pages → Re-run), or push an empty commit / `workflow_dispatch`.

## Agent discovery surfaces

Static artifacts served from `public/` (copied verbatim into `dist/`):

| Path | Purpose |
|---|---|
| `/.well-known/api-catalog` | RFC 9727 API catalog (`application/linkset+json`) |
| `/.well-known/openapi.json` | `service-desc` OpenAPI for the discovery surface |
| `/.well-known/agent-skills/index.json` | Agent Skills discovery index |
| `/skills/statefulmemory/SKILL.md` | Hosted skill artifact referenced by the index |
| `/auth.md` | Auth.md agent auth/registration doc |
| `/webmcp.js` | WebMCP `navigator.modelContext` tools (loaded from every page head) |

`skills/statefulmemory/SKILL.md` is copied to `public/skills/statefulmemory/SKILL.md`.
If the source skill changes, re-copy it and update the `digest` in
`public/.well-known/agent-skills/index.json`:

```bash
cp ../skills/statefulmemory/SKILL.md public/skills/statefulmemory/SKILL.md
shasum -a 256 public/skills/statefulmemory/SKILL.md   # sha256:<hex>
```

### Cloudflare (required — GitHub Pages cannot set these)

Two checks cannot be satisfied by static files on GitHub Pages, because they
depend on **response headers** and a non-default **Content-Type**. Applied at
Cloudflare (zone `statefulmemory.dev`) as a zone `http_response_headers_transform`
entrypoint ruleset — id `8d6860a081884a71a19db13b083caa7d` — with two rules
(homepage `Link` headers, and `content-type: application/linkset+json` for
`/.well-known/api-catalog`).

1. **Link headers on `/`** (`checks.discoverability.linkHeaders`) — a Transform
   Rule → *Modify Response Header* → *Add* for request `starts with /`:

   ```
   Link: </.well-known/api-catalog>; rel="api-catalog", </auth.md>; rel="describedby", </llms.txt>; rel="service-doc"
   ```

2. **Content-Type for `/.well-known/api-catalog`** — the file has no extension,
   so the origin serves it as `application/octet-stream`. Add a Transform Rule
   on response for `uri.path == "/.well-known/api-catalog"` setting
   `content-type: application/linkset+json`.

Verify with:

```bash
curl -sSI https://statefulmemory.dev/ | grep -i '^link:'
curl -sSI https://statefulmemory.dev/.well-known/api-catalog | grep -i '^content-type:'
```

### Markdown for Agents (`markdownNegotiation`)

Also a Cloudflare zone feature, not repo config, and it needs a **Pro or
Business** plan. Enable in the dashboard under **AI Crawl Control → Markdown
for Agents → On**, or via API with a token that has **Zone Settings: Edit**:

```bash
curl -X PATCH "https://api.cloudflare.com/client/v4/zones/${ZONE_ID}/settings/content_converter" \
  -H "Authorization: Bearer ${CF_API_TOKEN}" \
  -H 'Content-Type: application/json' \
  --data-raw '{"value":"on"}'
```

Verify:

```bash
curl -sSI -H 'Accept: text/markdown' https://statefulmemory.dev/ | grep -i '^content-type:'
# → content-type: text/markdown; charset=utf-8
```

`Content-Signal` is declared in `public/robots.txt` and is preserved on the
converted Markdown response.

### OAuth / OIDC discovery (`oauthDiscovery`, `oauthProtectedResource`)

**Not published, intentionally.** statefulmemory has no hosted OAuth/OIDC
authorization server — team authentication is bearer tokens minted by
`statefulmemory team token-create` over TCP+TLS. Publishing
`/.well-known/oauth-authorization-server` or an OpenID configuration with
endpoints that do not exist would be inaccurate and would send agents at broken
URLs. `/auth.md` documents the real model instead. If a hosted authorization
server is added later (e.g. for StatefulMemory Cloud or Cloudflare Access),
publish consistent Protected Resource Metadata + Authorization Server metadata
and revisit this.

