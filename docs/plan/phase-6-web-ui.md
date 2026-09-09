# Phase 6 — Web UI

**Goal.** A dashboard a non-technical person can use, with no JavaScript build
step.

**Demo.** Someone who has never seen a terminal logs in, looks at their VMs,
creates one from a form, and watches the apply progress live. The change appears
as a Git commit.

**Estimate.** 2–3 weeks.

**Rust you will learn.** Macros (`maud!`), `rust-embed`, streaming responses
(SSE), form handling, CSRF.

---

## Step 6.1 — Maud and the layout

```rust
// crates/dae-web/src/layout.rs
use maud::{html, Markup, DOCTYPE};

pub fn page(title: &str, user: &Principal, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) " — Daedalus" }
                link rel="stylesheet" href="/static/app.css";
                script src="/static/htmx.min.js" defer {}
            }
            body {
                (nav(user))
                main .container { (body) }
            }
        }
    }
}
```

💡 `maud!` is a procedural macro that checks your HTML **at compile time**. A
mistyped tag is a build error, and everything interpolated is auto-escaped
against XSS by default. To emit raw HTML you must explicitly say
`maud::PreEscaped` — which is exactly the right ergonomics: safe by default,
unsafe only on purpose.

**Check:** a page renders with correct structure and escapes `<script>` in a
resource name.

---

## Step 6.2 — Embed the assets

```rust
#[derive(rust_embed::RustEmbed)]
#[folder = "assets/"]
struct Assets;

async fn static_handler(Path(path): Path<String>) -> impl IntoResponse {
    match Assets::get(&path) {
        Some(f) => ([(CONTENT_TYPE, mime_for(&path)),
                     (CACHE_CONTROL, "public, max-age=31536000, immutable")],
                    f.data).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
```

Vendor `htmx.min.js` (~14KB) into `assets/`. Do not load it from a CDN — your
homelab dashboard should work when the internet does not.

💡 The whole UI now compiles into the binary. `scp daedalusd server:` is the
entire deployment. This is the payoff for
[D-008](../decisions.md#d-008--the-web-ui-is-server-rendered-with-htmx-not-a-spa).

⚠️ Use content-hashed filenames (`app.a3f9c2.css`) with the immutable cache
header, or browsers will serve stale CSS after an upgrade.

**Check:** the binary runs from an empty directory and serves CSS and JS.

---

## Step 6.3 — CSS design system

Hand-written, using custom properties. A dashboard is tables, forms, and badges.

```css
:root {
  --bg: #fafafa;  --surface: #fff;  --border: #e4e4e7;
  --text: #18181b; --muted: #71717a;
  --accent: #4f46e5;
  --ok: #16a34a; --warn: #ca8a04; --err: #dc2626;
  --radius: 6px;
  --font: ui-sans-serif, system-ui, -apple-system, sans-serif;
  --mono: ui-monospace, "SF Mono", Menlo, monospace;
}
@media (prefers-color-scheme: dark) {
  :root { --bg:#09090b; --surface:#18181b; --border:#27272a;
          --text:#fafafa; --muted:#a1a1aa; }
}
```

Components you actually need: `.table`, `.badge` (one per phase), `.btn`,
`.form-field`, `.card`, `.alert`, `.spinner`, `.diff` (for plans).

⚠️ Get dark mode right from the start via `prefers-color-scheme`. Retrofitting it
means auditing every colour you hard-coded.

**Check:** every page is readable in both light and dark, and at 375px wide.

---

## Step 6.4 — Resource list with htmx

```rust
html! {
    div #machines
        hx-get="/ui/tenants/acme/machines"
        hx-trigger="load, every 10s"
        hx-swap="innerHTML"
    { (spinner()) }
}
```

The endpoint returns an HTML **fragment**, not JSON. That is the htmx model: the
server owns rendering, and the client swaps DOM.

💡 The insight that makes htmx click: you already have the data and the
templates on the server. Serialising to JSON so JavaScript can rebuild the same
HTML is work you can simply skip.

⚠️ Polling every 10s per client will hammer your database with twenty tabs open.
Cache the rendered fragment briefly, or switch to SSE (step 6.6) for anything
that changes fast.

**Check:** the list refreshes without a full page reload; a VM's state change
appears within 10 seconds.

---

## Step 6.5 — Create a VM from a form

The form must be **guided**, not a YAML textarea. That is the entire point of
the web UI.

- **Class** — a `<select>` of catalog `MachineClass`es, showing CPU/RAM/disk
- **Image** — a `<select>` of catalog images
- **Network** — only the tenant's own networks
- **Name** — validated live with `hx-post="/ui/validate/name"`
- **Quota** — shown live: "After this VM: 24/48 vCPU, 80Gi/128Gi memory"

Submitting does **not** apply. It:

1. renders the manifest YAML
2. commits it to a branch (see [gitops.md](../gitops.md#write-back-how-the-web-ui-stays-honest))
3. computes a plan
4. shows the plan for confirmation
5. applies on confirmation

⚠️ CSRF protection is mandatory on every mutating form. Use a per-session token
in a hidden field, validated server-side. `SameSite=Lax` cookies help but are not
sufficient alone.

**Check:** creating a VM in the browser produces a Git commit with the right
trailers, then a real VM.

---

## Step 6.6 — Live plan and apply output

Server-sent events, which are far simpler than WebSockets for one-way streaming:

```rust
async fn apply_stream(scope: TenantScope, Path(id): Path<Uid>)
    -> Sse<impl Stream<Item = Result<Event, Infallible>>>
{
    let rx = state.engine.subscribe(id);
    Sse::new(BroadcastStream::new(rx).map(|ev| {
        Ok(Event::default().event("action").data(render_action(&ev?).into_string()))
    }))
    .keep_alive(KeepAlive::default())     // ⚠️ proxies kill idle connections
}
```

```html
<div hx-ext="sse" sse-connect="/ui/apply/01J8.../stream" sse-swap="action"
     hx-swap="beforeend"></div>
```

**Check:** applying in the browser streams each action's result as it happens,
and survives a slow apply behind a reverse proxy.

---

## Step 6.7 — The pages

| Route | Contents |
| --- | --- |
| `/` | Dashboard: counts by phase, drift, recent events, quota bars |
| `/machines` | List, filter, search |
| `/machines/{name}` | Detail: spec, status, events, drift, console, actions |
| `/machines/new` | Guided creation form |
| `/clusters` | Cluster list (Phase 7) |
| `/networks` | Networks and IPAM (Phase 8) |
| `/plan` | Current plan, apply button |
| `/events` | Filterable audit and event log |
| `/settings/tenant` | Members, repo, quota (read-only for non-owners) |
| `/admin/tenants` | Platform admin only |

⚠️ The UI must **hide** what a user cannot do, and the server must **still refuse
it**. Hiding a button is UX; the Cedar check is security. Never rely on the
former.

**Check:** log in as a `tenant-viewer` — no mutating buttons appear, and
hand-crafting the POST still returns 403.

---

## Step 6.8 — VM console

`xterm.js` (vendored) over a WebSocket proxying Proxmox's serial console
(`/nodes/{node}/qemu/{vmid}/termproxy`).

⚠️ Authorise the WebSocket upgrade with the same Cedar check as everything else.
WebSocket endpoints are a classic place where auth gets forgotten, because they
do not look like the other routes.

This is the one place a JS library is genuinely warranted — a terminal emulator
is not something to hand-roll.

**Check:** the console works and is refused for a user without console
permission.

---

## Step 6.9 — Accessibility and polish

- Semantic HTML: real `<table>`, `<label for>`, `<button>` not `<div onclick>`
- Keyboard navigable; visible focus rings
- `aria-live="polite"` on the streaming apply output
- Status conveyed by **icon and text**, not colour alone
- Empty states that tell you what to do next, not blank tables
- Error pages that say what went wrong and offer the correlation ID

💡 Semantic HTML gets you most of the way for free. Maud makes it natural to
write, since you are typing tags directly.

**Check:** navigate a full VM creation using only the keyboard. Run Lighthouse;
aim above 90 on accessibility.

---

## Definition of done

- [ ] Login via OIDC; sessions work; logout works
- [ ] Machine list, detail, and guided create form
- [ ] Creating a VM produces a Git commit, then a plan, then an apply
- [ ] Live streaming apply output via SSE
- [ ] Quota shown live during creation
- [ ] Dark mode; responsive to 375px
- [ ] CSRF protection on every mutating form
- [ ] Permissions hidden in UI **and** enforced server-side (tested)
- [ ] Assets embedded; the binary runs from an empty directory
- [ ] Keyboard navigable; Lighthouse accessibility > 90
- [ ] **Zero npm dependencies**

## Pitfalls

- **A YAML textarea instead of a form.** That is the CLI with extra steps, and it
  fails the actual goal of this phase.
- **Trusting the UI for authorisation.** Hidden buttons are not security.
- **Forgetting CSRF.** htmx sends normal form posts; they are forgeable.
- **Polling too aggressively.** Twenty open tabs will find your N+1 queries.
- **No SSE keep-alive.** Reverse proxies drop idle connections at ~60s.
- **Hard-coded colours.** Dark mode retrofits are miserable.
- **Reaching for a JS framework** the first time something is awkward. Try the
  htmx way first; it is usually simpler than it looks.
