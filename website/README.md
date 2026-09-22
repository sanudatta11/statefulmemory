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
