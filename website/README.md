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

Do these once after the first successful `Pages` workflow on `main`:

1. **GitHub Pages**
   - Repo → Settings → Pages
   - Source: **GitHub Actions**
   - Custom domain: `memlayer.org`
   - Enable **Enforce HTTPS**
2. **DNS at your registrar** (GitHub’s current apex targets — confirm in Pages settings if they change)
   - Apex `memlayer.org`: A records to GitHub Pages IPs (shown in repo Pages settings)
   - `www.memlayer.org`: CNAME → `sanudatta11.github.io`
   - Prefer apex as canonical; redirect www → apex when Pages offers it
3. **Google Search Console**
   - Add a **Domain** property for `memlayer.org` (or URL-prefix `https://memlayer.org/`)
   - Verify ownership with a **DNS TXT** record at the registrar
   - Submit sitemap: `https://memlayer.org/sitemap-index.xml`
4. Confirm crawlability
   - `https://memlayer.org/robots.txt` lists the sitemap
   - `https://memlayer.org/sitemap-index.xml` returns 200 after deploy

`public/CNAME` already contains `memlayer.org` for the Pages custom domain.
