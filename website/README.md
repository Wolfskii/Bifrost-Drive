# Bifrost Drive Website

Static public product site for Bifrost Drive. It has no backend, analytics, cookies, account system, or runtime environment variables.

## Public URLs

After assigning the Dokploy HTTPS domain, these URLs are intended for Google Auth Platform:

- Application home page: `https://YOUR-DOMAIN.example/`
- Privacy policy: `https://YOUR-DOMAIN.example/privacy/`
- Terms of service: `https://YOUR-DOMAIN.example/terms/`

All three pages are public and require no authentication.

## Local Preview

From the repository root:

```bash
python -m http.server 8080 --directory website
```

Open <http://localhost:8080/>.

Run the OS/download resolver tests:

```bash
npm --prefix website test
```

## Docker and Dokploy

Build the image locally:

```bash
docker compose -f website/docker-compose.yml up --build
```

The Compose service exposes internal container port `8080` for Dokploy and does not publish a host port. For a local browser preview, use the Python preview command above, or run the image with an explicit temporary host mapping:

```bash
docker run --rm -p 8080:8080 bifrost-website
```

Then verify:

```bash
curl --fail http://localhost:8080/healthz
curl --fail http://localhost:8080/privacy/
curl --fail http://localhost:8080/terms/
```

In Dokploy:

1. Create a Compose application from this GitHub repository.
2. Set the Compose path to `website/docker-compose.yml`.
3. Deploy the `bifrost-website` service.
4. Configure the public domain to target container port `8080`.
5. Enable HTTPS and HTTP-to-HTTPS redirect in Dokploy/Traefik.
6. Keep `WEBSITE_PORT` unset unless you also need host-port access; Dokploy should route to container port `8080`.
7. Verify `/`, `/privacy/`, `/terms/`, and `/healthz` over HTTPS.

No secrets are required for the website container.

## Final Domain Checklist

When the final domain is known:

1. Replace `YOUR-DOMAIN.example` in `sitemap.xml.example` and rename it to `sitemap.xml`.
2. Add `Sitemap: https://YOUR-DOMAIN.example/sitemap.xml` to `robots.txt`.
3. Add canonical and `og:url` metadata to `index.html`, `privacy/index.html`, and `terms/index.html`.
4. Add the domain in Google Search Console and Google Auth Platform.
5. Enter the exact public home, privacy, and terms URLs in the OAuth consent configuration.
6. Confirm the publisher name and contact email are correct before submitting for verification.

## Replaceable Visuals

The current product visuals are SVG placeholders and are intentionally easy to replace:

- `assets/product-placeholder.svg`: hero application preview. Recommended replacement: 1440 x 900 WebP or PNG.
- `assets/drive-placeholder.svg`: native filesystem preview. Recommended replacement: 1440 x 960 WebP or PNG.
- `assets/bifrost.svg`: product mark copied from the desktop application assets.
- `assets/webble-on-dark.png`: supplied white Webble mark for dark surfaces.
- `assets/webble-on-light.png`: supplied dark Webble mark for light surfaces.

After replacing a file with a differently named asset, update its reference in `index.html`. Keep screenshots free of real names, email addresses, storage paths, tokens, and private file names.

## Download Behavior

`assets/site.js` fetches the latest public release from GitHub's CORS-enabled API:

`https://api.github.com/repos/Wolfskii/Bifrost-Drive/releases/latest`

- Windows receives the `windows-x86_64` updater artifact.
- Linux receives the `linux-x86_64` AppImage. RPM and alternate formats remain available through the **Other operating systems and package formats** link.
- macOS displays **Coming soon** and does not present an unavailable download.
- Mobile, unknown platforms, network errors, and missing metadata fall back to the latest GitHub Release page.

The behavior is covered by `tests/download-resolver.test.mjs`.

## Legal Review

The Privacy Policy and Terms reflect the current repository architecture and Google Drive integration. They are product-ready technical drafts, not jurisdiction-specific legal advice. Review them with qualified counsel before relying on them for a commercial launch.
