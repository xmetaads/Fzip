# fzip.org — the website

Static, self-contained, no build step. The fonts are system stacks, the favicon
is an inline SVG data URI, the icons are one inline sprite, and there are no
external requests at all.

```
vercel.json       deployment config — the whole setup lives here
.vercelignore     keeps the Rust project out of the upload
web/              everything that gets served
  index.html
  usage.md        full command reference
  llms.txt        summary written for language models
  robots.txt
  sitemap.xml
  favicon.ico
  fzip-mark-512.png
```

This file is deliberately **outside** `web/`, so maintainer notes are not
published alongside the site.

## Deploying on Vercel

Import `github.com/xmetaads/Fzip` and press Deploy. Nothing needs configuring in
the dashboard — `vercel.json` at the repository root already declares everything:

| Setting | Value | Why |
|---|---|---|
| `outputDirectory` | `web` | The repository root is a Rust project; only `web/` is the site. |
| `buildCommand` | `null` | There is nothing to build. Static files are served as they are. |
| `installCommand` | `null` | No `package.json`, no dependencies. Skipping install removes any chance of Vercel trying to guess. |
| `framework` | `null` | Stops framework auto-detection. |

If you would rather configure it in the dashboard instead, the equivalent is
**Root Directory → `web`**, framework preset **Other**, and both Build and
Install commands left empty. Do not do both; `vercel.json` wins and the two can
disagree confusingly.

### What the config also sets

- **`Content-Type: text/plain; charset=utf-8`** on `usage.md` and `llms.txt` so
  they render in a browser instead of downloading. They are Markdown by content,
  but `text/markdown` makes browsers save the file, which defeats the point of
  linking them.
- **Security headers** on every route: `nosniff`, a referrer policy, frame
  options, and a Content-Security-Policy. The CSP allows `'unsafe-inline'` for
  styles and scripts because the page is deliberately one self-contained file
  with no external origins — there is nothing for an attacker to inject from.
- **Cache policy**: images cached for a week with `stale-while-revalidate`, but
  the HTML and the two text documents always revalidated, so a new deploy is
  visible immediately.
- **Short links**: `/docs` → the command reference, `/github` → the repository.

### Custom domain

Add `fzip.org` under **Settings → Domains**. Vercel will show the DNS records to
create — an `A` record for the apex or a `CNAME` for `www`. The site already
declares `https://fzip.org/` as its canonical URL, in `<link rel="canonical">`,
the Open Graph tags, `sitemap.xml` and the JSON-LD, so nothing else needs
changing once the domain resolves.

There is **one** build. The `--no-default-features` variant is no longer
published.

## The download

The public link is on our own domain, the way every desktop app does it:

```
https://fzip.org/download/fzip.exe
```

It is a **307 redirect** to the latest GitHub release asset. For a download that
is invisible to the user — the browser follows it in the background and saves
the file; the page they are on never navigates, so the address bar never leaves
`fzip.org`. Meanwhile GitHub serves the bytes, so the download costs us no
bandwidth at all.

| Path | Goes to |
|---|---|
| `/download/fzip.exe` | the newest release, always |
| `/download/fzip-1.0.0-x64.exe` | the `Fzip` tag specifically, so a pinned link never breaks |
| `/releases` | the releases page |
| `/github` | the repository |

The redirect is deliberately `"permanent": false` — a 308 would be cached by
browsers and would pin people to whichever release happened to be current the
first time they visited.

The redirect response also carries the build identity, so a script can check the
current version without downloading 2.8 MB:

```powershell
(Invoke-WebRequest https://fzip.org/download/fzip.exe -MaximumRedirection 0 `
   -SkipHttpErrorCheck).Headers | Where-Object Key -like "X-Fzip-*"
```

### Publishing a new version

1. `cargo build --release`
2. Draft a release on GitHub and attach `target/release/fzip.exe`. **The asset
   must keep the exact name `fzip.exe`** — the `latest/download/fzip.exe` URL
   resolves by filename, so renaming it breaks every existing link.
3. Add a pinned redirect for the new tag in `vercel.json`, alongside the
   existing one, so old versioned links keep working.
4. Update the SHA-256 and byte size in four places: `X-Fzip-*` in `vercel.json`,
   the `.hash` block and JSON-LD `fileSize` in `web/index.html`, and the
   verification sections of `usage.md` and `llms.txt`.
5. Commit and push. Vercel deploys on push.

Verify what you published actually matches what the site claims:

```powershell
$tmp = "$env:TEMP\fzip-check.exe"
Invoke-WebRequest https://fzip.org/download/fzip.exe -OutFile $tmp
(Get-FileHash $tmp -Algorithm SHA256).Hash
```

That should equal the hash on the download card. It is worth doing every
release — uploading yesterday's build is an easy mistake, and the published hash
is what people use to decide whether to trust the file.

### If you ever make the repository private again

Release assets on a private repository return 404 to everyone without access —
and it will still look fine to you, because your browser is signed in. Switch
the redirect back to a rewrite serving `web/download/` from the deployment, and
drop `/web/download/` out of `.gitignore`.

## Legacy: wiring the link to an external host

The binary is deliberately **not** committed to this repository — a 2.8 MB
executable in git history is dead weight, and every rebuild would add another
copy. Pick one of the two routes below, then update the four places listed
underneath.

### Route A — GitHub Releases (recommended)

Releases give you a permanent URL per version, a download counter, and no repo
bloat.

1. Build: `cargo build --release`
2. On GitHub: **Releases → Draft a new release**, tag `v1.0.0`, attach
   `target/release/fzip.exe` renamed to `fzip-1.0.0-x64.exe`.
3. Your download URL is then:

```
https://github.com/xmetaads/Fzip/releases/download/v1.0.0/fzip-1.0.0-x64.exe
```

A URL that always points at the newest release, so the site needs no edit when
you publish v1.0.1:

```
https://github.com/xmetaads/Fzip/releases/latest/download/fzip-1.0.0-x64.exe
```

The filename in a `latest` URL must stay the same across releases, so decide
early whether to keep the version in the filename. Using a plain `fzip.exe`
asset name makes `latest` work forever.

### Route B — serve the file from the site

Put the executable at `web/download/fzip-1.0.0-x64.exe`. That path is in
`.gitignore`, so copy it in as part of your deploy step rather than committing
it. Set these headers so browsers download instead of trying to display it:

```
Content-Type: application/octet-stream
Content-Disposition: attachment
```

### The four places to change

Whichever route you take, the URL appears in four files. They must agree,
because assistants read the last two directly.

| File | What to change |
|---|---|
| `web/index.html` | The `href` on the download button in `#download` |
| `web/index.html` | `"downloadUrl"` inside the JSON-LD block in `<head>` |
| `web/llms.txt` | The `Download:` line at the bottom |
| `web/usage.md` | Any download reference in the verification section |

One command finds every occurrence:

```powershell
Select-String -Path web\* -Pattern "download/fzip" -AllMatches
```

### When you publish a new version

The SHA-256 and byte size change with every build, even for identical source.
Update them together in `web/index.html` (the `.hash` block and the JSON-LD
`fileSize`), `web/usage.md` and `web/llms.txt`. Get the current values with:

```powershell
Get-FileHash target\release\fzip.exe -Algorithm SHA256
(Get-Item target\release\fzip.exe).Length
```

## Deploying

Any static host: Cloudflare Pages, Netlify, GitHub Pages, S3, nginx. Serve
`index.html` at `/`. Two headers on the executable so browsers download rather
than try to display it:

```
Content-Type: application/octet-stream
Content-Disposition: attachment
```

Serve `usage.md` as `text/markdown; charset=utf-8` and `llms.txt` as
`text/plain; charset=utf-8`.

## Written so an assistant can teach it

Fzip has no graphical interface, so most people meet it through a search or by
asking an assistant "how do I use this". The site is built to answer that
without a human in the loop:

| Layer | What it carries |
|---|---|
| **`llms.txt`** | A summary aimed squarely at models: the three commands that cover most needs, the full option table, exit codes, worked examples, and the misunderstanding to head off first — that double-clicking the exe is *supposed* to print help. |
| **`usage.md`** | The complete reference in plain Markdown: every command, every option, the four overwrite modes, batch and PowerShell patterns, the measured performance table, and the safety behaviour. |
| **JSON-LD on the page** | A `SoftwareApplication` with an eight-item `featureList`, two `HowTo` objects with numbered steps (extract, create-encrypted), and a five-question `FAQPage`. Assistants and search engines can lift working commands straight out of it. |
| **The HTML itself** | The command reference is real `<table>` markup with `<th scope="row">`, not styled divs, so it survives being converted to text. |

If you change a command or an option, change it in **all four** places. They are
the product's documentation, not decoration.

## Visual direction

Light-first. The page is meant to be read quickly by someone who is mildly
annoyed that the program has no window, so clarity beats atmosphere: solid white
cards on a very light slate ground, crisp one-pixel borders, restrained shadows,
and a single amber accent that only ever marks the thing you are meant to look
at next.

Every colour token carries its measured contrast against its own ground in a
comment; nothing is below 4.5:1. Both themes are defined at token level, so the
viewer's OS preference and the theme toggle both work.

Interaction follows the
[ui-ux-pro-max](https://github.com/nextlevelbuilder/ui-ux-pro-max-skill) presets:
hover 180ms `power1.out` with displacement capped at 2px, scroll reveal 520ms
`power2.out` from a 12px offset, staggered 60ms and capped at eight items. Touch
targets are 44px via one `--tap` token; `prefers-reduced-motion` disables the
reveal and the hero animation entirely.

## No competitor names

The page makes its speed argument with Fzip's own numbers — the same binary and
the same archive with only `-t` changed — rather than by naming other tools. That
is both a cleaner claim and a more honest one, since the parallel/sequential
difference *is* the actual explanation.

## Keeping the numbers honest

Every figure is reproducible. The worker-count table came from running
`fzip t` at each thread count, five runs each, taking the best. Update these
together if you re-measure:

| What | Where |
|---|---|
| Hero throughput, race times | `.hero h1`, the two `.runner` blocks |
| Worker-count chart | `.chart` in `#speed`, and the table in `usage.md` |
| Test count, VirusTotal result | the `.bento` block in `#speed` |
| Version, file size, SHA-256 | `#download`, plus `usage.md` and `llms.txt` |

The "what you actually see on disk" note and the whole `#limits` section exist on
purpose. A page that only lists wins reads as marketing; one that publishes the
ceiling next to the peak reads as measurement. Keep them.
