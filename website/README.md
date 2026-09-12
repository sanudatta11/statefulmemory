# memlayer.org

Public docs site for [memlayer](https://github.com/sanudatta11/memlayer), built with
[Astro Starlight](https://starlight.astro.build/) and deployed to GitHub Pages.

Canonical URL: **https://memlayer.org**

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
   - Custom domain: `memlayer.org`
   - Enable **Enforce HTTPS**
3. **DNS at your registrar** (confirm IPs in Pages settings if they change)
   - Apex `memlayer.org`: A records to GitHub Pages IPs
   - `www.memlayer.org`: CNAME → `sanudatta11.github.io`
   - Prefer apex as canonical; redirect www → apex when Pages offers it
4. **Google Search Console**
   - Add a **Domain** property for `memlayer.org` (or URL-prefix `https://memlayer.org/`)
   - Verify ownership with a **DNS TXT** record at the registrar
   - Submit sitemap: `https://memlayer.org/sitemap-index.xml`
5. Confirm crawlability
   - `https://memlayer.org/robots.txt` lists the sitemap and allows AI search bots
   - `https://memlayer.org/sitemap-index.xml` returns 200 after deploy
   - `sitemap-0.xml` lists every docs page (home + getting-started, install,
     agents, commands, config, locomo, troubleshooting); `/404` is excluded
6. Confirm LLM-ready surfaces
   - `https://memlayer.org/llms.txt` (curated index)
   - `https://memlayer.org/llms-full.txt` (plain-text product summary)
   - Repo root `AGENTS.md` / `CLAUDE.md` for coding agents cloning the source

`public/CNAME` already contains `memlayer.org` for the Pages custom domain.

After enabling Pages, re-run the failed **Pages** workflow on `main` (Actions → Pages → Re-run), or push an empty commit / `workflow_dispatch`.
