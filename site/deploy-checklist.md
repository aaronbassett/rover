# Netlify deploy checklist (manual — needs your Netlify + DNS access)

The site is built and configured. `site/netlify.toml` already pins the build
settings, so most of this is clicking through Netlify + pointing DNS.

## 1. Connect the repo
1. Netlify → **Add new site → Import from Git** → pick the `rover` repo.
2. Build settings are read from `site/netlify.toml`:
   - **Base directory:** `site`
   - **Build command:** `pnpm build`
   - **Publish directory:** `site/build` (`build`, relative to base)
   - Netlify auto-detects pnpm from `pnpm-lock.yaml` + `packageManager` in `site/package.json`.
3. Deploy. Confirm the **deploy preview** renders the landing + docs.

## 2. Node version (one decision)
`netlify.toml` pins `NODE_VERSION = "26.3.1"` — the version you asked for and
have locally. It is a real, current release and Netlify can fetch it. **Note:**
Node 26 is a non-LTS ("Current") line. If you'd prefer a longer support window
for a docs site, change that one line to the latest LTS (e.g. `24.17.0`); the
site has no dependency on Node 26 specifically (`package.json` engines say
`>=20`). Your call — left at 26.3.1 as specified.

## 3. Custom domain + TLS
1. Site → **Domain management → Add a domain** → `rover-fetch.com`.
2. Add a `www` → apex redirect (Netlify offers this automatically).
3. Point DNS at Netlify: either move the domain to **Netlify DNS**, or at your
   current registrar add the records Netlify shows (an `A`/`ALIAS` for the apex
   + `CNAME` for `www`).
4. Enable **HTTPS** (Let's Encrypt) and auto-renew.

## 4. Verify the agent contract in production
```sh
curl -sI https://rover-fetch.com/docs/cli.md | grep -i content-type   # text/markdown; charset=utf-8
curl -s  https://rover-fetch.com/llms.txt | head                       # lists the doc pages
curl -s  https://rover-fetch.com/docs/cli | grep -q "rover" && echo OK  # full static HTML
```

## 5. Go live
- **Production deploys** happen automatically on merge to `main` (Netlify Git
  integration). Merge `feat/docs-site` → `main` once the preview looks right and
  DNS/TLS are green.
- Deploy **previews** run on every PR.

## Optional polish (not blockers)
- **Social/OG card:** there is intentionally no `image:` in `themeConfig` (the
  Docusaurus scaffold card was removed). For nicer link unfurls, generate a
  Rover-branded ~1.91:1 card, drop it in `site/static/img/`, and set
  `themeConfig.image`.
- `onBrokenMarkdownLinks` currently sits at the top level of
  `docusaurus.config.ts` (works; prints a Docusaurus 3.x deprecation warning) —
  move it to `markdown.hooks.onBrokenMarkdownLinks` whenever convenient.
