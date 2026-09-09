# Phase 5 — API, authentication, and multi-tenancy

**Goal.** An HTTP API with real authentication, Cedar authorisation,
type-enforced tenant scoping, and quotas. The CLI becomes a client.

**Demo.** Two users, two tenants. User A cannot see, touch, or even enumerate
user B's resources — and you can demonstrate why at four independent layers.

**Estimate.** 3–4 weeks. The most security-critical phase.

**Rust you will learn.** Axum extractors, tower middleware, the **typestate
pattern**, `Send`/`Sync` bounds in practice, OIDC flows.

---

## Step 5.1 — The API skeleton

```rust
// crates/dae-api/src/lib.rs
pub fn router(state: AppState) -> Router {
    Router::new()
        .nest("/api/v1", api_v1())
        .route("/healthz", get(healthz))
        .route("/readyz",  get(readyz))
        .route("/metrics", get(metrics))
        .layer(TraceLayer::new_for_http())
        .layer(TimeoutLayer::new(Duration::from_secs(30)))
        .layer(CompressionLayer::new())
        .with_state(state)
}

fn api_v1() -> Router<AppState> {
    Router::new()
        .route("/tenants",                    get(list_tenants).post(create_tenant))
        .route("/tenants/{tenant}/machines",  get(list_machines))
        .route("/tenants/{tenant}/machines/{name}",
               get(get_machine).delete(delete_machine))
        .route("/tenants/{tenant}/plan",      post(create_plan))
        .route("/tenants/{tenant}/apply",     post(apply_plan))
        .route("/tenants/{tenant}/events",    get(list_events))
        .route_layer(middleware::from_fn(require_auth))   // ⚠️ see 5.4
}
```

⚠️ `/healthz`, `/readyz` and `/metrics` sit **outside** the auth layer, on
purpose. Everything else is inside it. Structure the router so it is impossible
to add an unauthenticated business endpoint by accident — that is why `api_v1()`
applies the layer to the whole subtree rather than per route.

**Check:** `curl /healthz` works unauthenticated; `curl /api/v1/tenants` returns
401.

---

## Step 5.2 — Errors as `problem+json`

```rust
#[derive(Serialize)]
pub struct ProblemDetails {
    #[serde(rename = "type")] pub type_uri: String,
    pub title:  String,
    pub status: u16,
    pub detail: String,
    pub instance: Option<String>,
    pub correlation_id: String,          // ties to Ariadne
}

impl IntoResponse for ApiError { /* map variants to status + problem */ }
```

⚠️ **Never leak internals.** `sqlx::Error` must not reach a client. Map to a
generic 500, log the real error with the correlation ID, and return that ID so
you can find it. And be careful that a 404 vs 403 distinction does not leak
existence across tenants — return **404** for a resource in another tenant, not
403, or you have built an enumeration oracle.

**Check:** a database error returns a clean 500 with a correlation ID that
appears in the logs. A cross-tenant read returns 404, not 403.

---

## Step 5.3 — Authentication

Two mechanisms, one output.

**OIDC for humans** (`openidconnect` crate): standard authorisation-code flow
with PKCE, discovery, JWKS caching and rotation. Session cookie is
`HttpOnly; Secure; SameSite=Lax`.

**Bearer tokens for machines:**

```rust
pub struct ApiToken {
    pub id:         Uid,
    pub tenant_id:  TenantId,
    pub hash:       String,        // sha256 of the secret — never the secret
    pub scopes:     Vec<Scope>,
    pub expires_at: Option<DateTime<Utc>>,
}
```

- Format `dae_sa_<32 random bytes, base62>` — the prefix makes leaked tokens
  greppable in logs and repos.
- Show the secret **once**, at creation. Store only the hash.
- Compare with a constant-time comparison (`subtle::ConstantTimeEq`).

⚠️ Do not use bcrypt/argon2 for API tokens. They are high-entropy random values,
not passwords, so a plain SHA-256 is correct and fast — you will verify one on
every request, and a deliberately slow KDF there is a self-inflicted DoS.

**Check:** both flows produce a `Principal`. An expired token is rejected. A
revoked token stops working immediately.

---

## Step 5.4 — The `TenantScope` typestate ★

The core of [D-007](../decisions.md#d-007--tenant-scoping-is-enforced-by-the-type-system).

```rust
// crates/dae-store/src/scope.rs

/// Proof that the current principal is authorised to act within this tenant.
/// The private field means no other module can construct one literally.
#[derive(Debug, Clone)]
pub struct TenantScope {
    tenant_id: TenantId,
    principal: PrincipalId,
    _seal: PhantomData<()>,
}

impl TenantScope {
    /// Only the auth layer may call this. `pub(crate)` plus a module
    /// boundary is what makes the guarantee hold.
    pub(crate) fn new(tenant_id: TenantId, principal: PrincipalId) -> Self {
        Self { tenant_id, principal, _seal: PhantomData }
    }
    pub fn tenant_id(&self) -> TenantId { self.tenant_id }
}
```

Then rewrite every store method from Phase 2:

```rust
impl ResourceRepo {
    pub async fn list(&self, scope: &TenantScope, kind: Option<Kind>)
        -> Result<Vec<StoredResource>>
    {
        sqlx::query_as!(StoredResource,
            "SELECT * FROM resources WHERE tenant_id = ?",   // always
            scope.tenant_id())
            .fetch_all(&self.pool).await
    }
}
```

And make it an axum extractor, so handlers receive it for free:

```rust
#[async_trait]
impl FromRequestParts<AppState> for TenantScope {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState)
        -> Result<Self, Self::Rejection>
    {
        let principal = parts.extensions.get::<Principal>()
            .ok_or(ApiError::Unauthenticated)?;
        let tenant = extract_tenant_from_path_or_header(parts)?;
        // Cedar decides. If it denies, no scope exists, so no query can run.
        state.policy.authorize(principal, Action::TenantAccess, &tenant)?;
        Ok(TenantScope::new(tenant.id, principal.id))
    }
}

// A handler cannot forget to scope — the type system supplied it.
async fn list_machines(scope: TenantScope, State(st): State<AppState>)
    -> Result<Json<Vec<MachineView>>, ApiError>
{
    Ok(Json(st.resources.list(&scope, Some(Kind::Machine)).await?.into()))
}
```

💡 **This is the best Rust lesson in the project.** The most likely
multi-tenancy bug — a forgotten `WHERE tenant_id = ?` — is now a compile error.
It cannot be written. Spend the time to get this right; it is why the project is
in Rust.

⚠️ Audit for holes: `grep` for any `sqlx::query` in the store crate that does not
take a scope. Add a CI check for `tenant_id` appearing in every query touching a
tenant table. The guarantee is only as strong as its weakest bypass.

**Check:** delete the `WHERE tenant_id = ?` from a query — it should not compile,
because `scope` becomes unused and the lint denies it. Add a test proving tenant
A's list never contains tenant B's resources.

---

## Step 5.5 — Cedar authorisation

```rust
pub struct PolicyEngine {
    policies: cedar_policy::PolicySet,
    schema:   cedar_policy::Schema,
    entities: EntityProvider,
}

impl PolicyEngine {
    pub fn authorize(&self, p: &Principal, a: Action, r: &ResourceRef)
        -> Result<(), ApiError>
    {
        let req = Request::new(p.into(), a.into(), r.into(), context, &self.schema)?;
        match self.authorizer.is_authorized(&req, &self.policies, &self.entities) {
            Decision::Allow => Ok(()),
            Decision::Deny  => {
                // ⚠️ Audit denials as loudly as successes.
                warn!(principal = %p.id, action = ?a, resource = %r, "denied");
                self.events.record_denial(p, a, r);
                Err(ApiError::Forbidden)
            }
        }
    }
}
```

Policies live in Git (`platform/policies/`) and reload on change. See
[multitenancy.md](../multitenancy.md#layer-2--authorisation) for examples.

Write `dae policy test` with a fixture file:

```yaml
- name: operator can create in own tenant
  principal: { id: alice, role: tenant-operator, tenant: acme }
  action: machine:create
  resource: { type: Machine, tenant: acme }
  expect: allow

- name: operator cannot touch another tenant
  principal: { id: alice, role: tenant-operator, tenant: acme }
  action: machine:create
  resource: { type: Machine, tenant: bob }
  expect: deny
```

💡 Policy without tests is decoration. Run these in CI.

**Check:** the fixture suite passes; a `forbid` rule overrides a matching
`permit` (Cedar guarantees this — write the test that proves you understand it).

---

## Step 5.6 — Quotas

```rust
pub struct QuotaChecker { /* ... */ }

impl QuotaChecker {
    /// Evaluated on the POST-APPLY footprint, not the delta.
    pub async fn check(&self, scope: &TenantScope, plan: &Plan)
        -> Result<QuotaReport, QuotaExceeded>;
}
```

Checked twice: at plan time (fail early with a readable message) and again at
apply time **inside the tenant's serialised apply lock**, so two concurrent plans
cannot both pass and jointly exceed.

```
error: quota exceeded for tenant 'acme'
  vcpu:    would be 52, limit is 48   (+4 over)
  memory:  would be 96Gi, limit 128Gi  ✓
Reduce the plan or ask a platform admin to raise the limit.
```

**Check:** a plan exceeding vCPU is rejected at plan time. Two concurrent applies
that would jointly exceed — exactly one succeeds.

---

## Step 5.7 — Tenant provisioning

Implement the lifecycle from
[multitenancy.md](../multitenancy.md#tenant-lifecycle) as a **resumable
workflow**, not a function:

```rust
pub enum ProvisionStep {
    AllocateId, GenerateKeys, AllocateVlan, CreatePool, CreateStorage,
    CreateSdnVnet, CreateUser, CreateToken, ApplyAcls, CreateGitRepo,
    SeedRepo, SetQuota, BindOwner, Complete,
}
```

Persist the completed step after each one. On restart, resume from where it
stopped.

⚠️ Step `CreatePool` **will** fail sometimes — Proxmox unreachable, a name
collision, a network blip. Partial tenant creation is a state the system must
handle gracefully, not an edge case. Every step must be idempotent (re-running
`CreatePool` for an existing pool succeeds).

**Check:** kill `daedalusd` midway through tenant creation; on restart it resumes
and completes. Run creation twice for the same name — the second is a clean
no-op, not a duplicate.

---

## Step 5.8 — Audit log

Every mutation, every denial, every cross-tenant read, in the schema from
[multitenancy.md](../multitenancy.md#layer-7--audit).

⚠️ There is **no delete endpoint at any privilege level**. Retention is handled
by compaction that snapshots and archives, never by erasure. An audit log an
administrator can quietly edit is not an audit log.

**Check:** `dae audit --tenant acme --since 24h` shows a complete record. There is
no API path that removes an event.

---

## Step 5.9 — The CLI becomes a client

Refactor `dae` to speak HTTP instead of touching the database. `dae-client` holds
the shared HTTP client, used by both the CLI and the integration tests.

Keep `--local` for `validate` and offline `plan` only. `--local` **cannot apply**
— applying requires the server, because that is where authorisation, quota, and
audit live. Keeping that boundary sharp is what stops the CLI from becoming a
second, unenforced path to your infrastructure.

**Check:** `dae get machines` works against a running server with a real token.
`dae apply --local` is rejected with a clear explanation.

---

## Step 5.10 — OpenAPI

Annotate handlers with `utoipa`, serve the spec at `/api/v1/openapi.json` and
Swagger UI at `/api/v1/docs`.

💡 Generating the spec from the handlers keeps documentation honest — it cannot
drift, because it is derived from the code that serves the requests.

**Check:** the spec validates, and Swagger UI can execute an authenticated request.

---

## Step 5.11 — Prove the isolation

Write these as permanent integration tests. They are the phase's real deliverable.

```rust
#[tokio::test] async fn tenant_a_cannot_list_tenant_b_machines() { }
#[tokio::test] async fn tenant_a_cannot_get_b_machine_by_uid()   { }  // 404, not 403
#[tokio::test] async fn tenant_a_cannot_reference_b_network()    { }
#[tokio::test] async fn tenant_a_token_gets_403_from_proxmox()   { }  // layer 6
#[tokio::test] async fn tenant_a_cannot_decrypt_b_secret()       { }
#[tokio::test] async fn quota_is_enforced_under_concurrency()    { }
#[tokio::test] async fn denials_are_audited()                    { }
#[tokio::test] async fn platform_admin_cross_tenant_read_is_audited() { }
```

**Check:** all pass, in CI, on every commit. Never let one be skipped or
`#[ignore]`d — a disabled isolation test is worse than none, because it looks
like coverage.

---

## Definition of done

- [ ] OIDC login works end to end
- [ ] API tokens: hashed, prefixed, scoped, expiring, revocable
- [ ] `TenantScope` extractor; **no store method omits it**
- [ ] Cedar authorises every request; policies are Git-managed and tested
- [ ] Quotas enforced at plan and apply, safe under concurrency
- [ ] Tenant provisioning is resumable and idempotent
- [ ] Audit log complete; no delete path exists
- [ ] CLI is a pure API client; `--local` cannot apply
- [ ] OpenAPI served and accurate
- [ ] All eight isolation tests pass in CI

## Pitfalls

- **Checking authorisation in handlers instead of extractors.** One forgotten
  handler is a breach. The extractor makes it structural.
- **Returning 403 for another tenant's resource.** That confirms existence. 404.
- **Leaking `sqlx::Error` to clients.** Table names in error messages are a gift
  to an attacker.
- **Argon2 for API tokens.** Wrong tool; a self-inflicted DoS on every request.
- **Logging tokens or secrets.** Use `secrecy::SecretString` so `Debug` cannot
  print them by accident.
- **Assuming tenant provisioning succeeds.** It will not. Make it resumable.
- **Adding an endpoint outside the auth subtree.** Structure the router so this
  is hard to do by accident.
