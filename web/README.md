# fzip.org — the website

Static, self-contained, no build step. The fonts are system stacks, the favicon
is an inline SVG data URI, the icons are one inline sprite, and there are no
external requests at all.

```
web/
  index.html      the site
  usage.md        full command reference, also served as a page
  llms.txt        summary written for language models
  download/
    fzip-1.0.0-x64.exe
```

There is **one** build. The `--no-default-features` variant is no longer
published.

## Wiring up the download link

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
