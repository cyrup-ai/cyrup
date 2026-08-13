# ADR-0010 — CFG-005: interactive credential acquisition for the four bespoke api-key providers

**Status** accepted (decided by default under the parity rule — overridable)
**Date** 2026-08-13
**Decides** OQ-8, the `CFG-005` half. (OQ-8 bundles two unrelated questions; the "~163
non-user-observable lows: work items or conformance suite?" half is **not** decided here and is not
affected by anything below.)
**Blocks released** batch 11 (`CFG-005` lands beside `PROV-003`, one diff); batch 12 (`PROV-030`'s
google-vertex end-to-end verification loses its hand-provisioned-credential precondition); the
`CFG-005` ledger row in `docs/gap-analysis/05-cyrup-config-and-resources.md`.
**Filename note** this file is `ADR-0010-oauth-acquisition.md`, and the slug is wrong — the residual
contains no OAuth (see the first false premise below). The path is kept stable because it is the
address other documents cite; the ADR is about **`ApiKeyAuth::login`**, not OAuth. Do not "fix" the
subject to match the slug.

## Context

### The question as posed rests on three false premises

The assignment, `PARITY-PLAN.md:1386` and `PARITY-PLAN.md:1478-1487`, and the `CFG-005` row at
`05-cyrup-config-and-resources.md:134` all describe the residual as *"the two multi-prompt api-key
login flows (cloudflare, google-vertex)"* and its consequence as *"two registered providers cannot
be authenticated interactively at all."* Read against both trees at HEAD `72cd292` / `v0.83.0`, all
three parts of that are wrong. The real question is bigger and worse, and it is the one decided
below.

**(1) None of this is OAuth.** The ADR filename says "oauth-acquisition"; the residual contains no
OAuth. Upstream types the missing members on `ApiKeyAuth`, not `OAuthAuth`
(`pi/packages/ai/src/auth/types.ts:161-186` @v0.83.0 — `interface ApiKeyAuth`, whose optional
`login?(interaction): Promise<ApiKeyCredential>` is `:166`).
cyrup's OAuth acquisition surface is essentially closed: `OAuthAuth::login` exists at
`crates/cyrup-provider/src/auth/mod.rs:124-131`, eleven flow modules live under
`crates/cyrup-provider/src/auth/oauth/` (14 849 lines across the directory: `anthropic.rs`, `github_copilot.rs`,
`kimi_coding.rs`, `openai_codex.rs`, `openrouter.rs`, `radius.rs`, `xai.rs` plus
`device_code/callback/pkce/page/query/sha256/random`), and the only OAuth-side hole left is
`PROV-029`'s wiring — a different item, already scheduled in batch 11, deliberately not touched
here.

**(2) There are four missing flow bodies across four registered provider ids, not two.** Upstream
@v0.83.0 defines exactly five `ApiKeyAuth.login` bodies:

| upstream body | prompts | cyrup status |
|---|---|---|
| `packages/ai/src/auth/helpers.ts:12-15` (`envApiKeyAuth`) | 1 secret | **ported** — `crates/cyrup-config/src/login.rs:343-351` (`env_api_key_login`) |
| `packages/ai/src/providers/cloudflare-auth.ts:50-54` (`cloudflareWorkersAIAuth`) | secret + text (account id) | **missing** |
| `packages/ai/src/providers/cloudflare-auth.ts:70-79` (`cloudflareAIGatewayAuth`) | secret + text + text (gateway id) | **missing** |
| `packages/ai/src/providers/google-vertex.ts:15-61` (`vertexAuth`) | select(3) → secret, or notify+link → text ×2-3 | **missing** |
| `packages/ai/src/providers/amazon-bedrock.ts:13-51` (`bedrockAuth`) | select(3) → secret, or notify+link → text | **missing** |

cyrup has exactly four bespoke `ApiKeyAuth` impls, and they are the same four: `CloudflareWorkersAiAuth`
(`providers/cloudflare.rs:74`), `CloudflareAiGatewayAuth` (`:144`), `GoogleVertexApiKeyAuth`
(`providers/google_vertex.rs:162`), `AmazonBedrockApiKeyAuth` (`providers/amazon_bedrock.rs:211`).
Every other provider uses `env_key` (login ported) or `keyless_local` (no login upstream either).
All four are registered in production: `providers/all.rs:167` (ai-gateway), `:171` (workers-ai),
`:177` (bedrock), `:187` (google-vertex), and `all_providers()` is what `/login` reads
(`crates/cyrup-tui/src/app.rs:2387-2390`).

**`amazon-bedrock` appears in no gap item at all.** `CFG-005` counts two; cyrup's own source counts
four in a doc comment — `crates/cyrup-config/src/login.rs:309-317` names "`envApiKeyAuth`
(`helpers.ts:12`), Cloudflare (`cloudflare-auth.ts:48`), Vertex (`google-vertex.ts:15`), Bedrock
(`amazon-bedrock.ts:13`)" — and `providers/amazon_bedrock.rs:65-72` documents the hole for
`bedrockAuth.login` verbatim. The ledger simply never picked it up.

**(3) The behaviour is not "cannot be authenticated interactively"; it is "runs the wrong flow and
silently stores a partial credential."** This is the decisive fact and it inverts the item's
severity. cyrup has no `login` member on `ApiKeyAuth` (`auth/mod.rs:59-71` — `name` + `resolve`
only), so `login()` answers pi's `if (!method?.login)` guard (`pi/packages/ai/src/models.ts:435`
@v0.83.0, inside `Models.login`, `:431-444`) by sniffing the strategy's display name:

```rust
// crates/cyrup-config/src/login.rs:316-318
pub fn api_key_strategy_supports_login(strategy_name: &str) -> bool {
    strategy_name != KEYLESS_LOCAL_STRATEGY
}
```

`"Cloudflare API key"`, `"Google Cloud credentials"` and `"AWS credentials or bearer token"` are all
`!= "keyless-local"`, so the predicate answers **true** for all four — and then the api-key arm of
`login()` runs the generic one-secret flow regardless of which strategy it is:

```rust
// crates/cyrup-config/src/login.rs:786-802
let Some(api_key) = provider.auth.api_key.as_ref()
    .filter(|s| api_key_strategy_supports_login(s.name()))
else { return Err(LoginError::Unsupported { … }) };
let label = api_key_login_label(api_key.name(), &provider.name);
env_api_key_login(&label, interaction).await?
```

So all four rows appear in the `/login` picker with `supports_login: true`
(`login.rs:465-478`), `start_provider_login` routes them to `LoginStep::ApiKey` rather than the
ambient dialog (`login.rs:641-663`), and the TUI drives them into
`cyrup_config::login::login` (`crates/cyrup-tui/src/app.rs:2666-2673`), which persists a
`Credential::ApiKey { key, env: None }`. Per provider, at HEAD:

| provider | what pi writes | what cyrup writes | user-visible result |
|---|---|---|---|
| `cloudflare-workers-ai` | `{key, env:{CLOUDFLARE_ACCOUNT_ID}}` | `{key}` | `resolve` needs both (`providers/cloudflare.rs:85-94`) → returns `None` → **provider reports "not configured" immediately after a login that reported success** |
| `cloudflare-ai-gateway` | `{key, env:{ACCOUNT_ID, GATEWAY_ID}}` | `{key}` | same dead end, two fields short (`providers/cloudflare.rs:155-167`) |
| `amazon-bedrock` | bearer **or** `{env:{AWS_PROFILE}}` **or** `{}` (credential chain) | `{key}` always | only the bearer rung is reachable; AWS-profile and credential-chain login are gone, and a user who pastes an access-key id gets a stored "bearer" that fails at SigV4 time |
| `google-vertex` | api key **or** `{env:{PROJECT, LOCATION[, APPLICATION_CREDENTIALS]}}` | `{key}` always | the api-key rung works by coincidence; the ADC and service-account rungs — and the `notify` carrying the `gcloud auth application-default login` instruction and the ADC doc link (`google-vertex.ts:34-46`) — are unreachable |

That is a lying control, not a missing feature: the picker advertises a login, the dialog reports
success, and the credential store ends up in a state the provider cannot use. It is precisely the
class batch 3's `lint-unwired` exists to stop shipping.

**Compounding defect, cloudflare only, currently filed nowhere.** pi's `resolveValue` falls back to
ambient env **even when a credential exists** — `return fromCredential ?? (await ctx.env(name));`
(`pi/packages/ai/src/providers/cloudflare-auth.ts:18-23` @v0.83.0), with the intent spelled out in
its own comment at `:15-17`: *"A credential carrying only the API key must still pick up the account
/ gateway id from the environment."* cyrup's port drops that fallback — the `Some(Credential::ApiKey…)`
arm returns `None` without consulting `ctx` (`crates/cyrup-provider/src/providers/cloudflare.rs:48-65`).
So after the wrong-flow login above, exporting `CLOUDFLARE_ACCOUNT_ID` **does not** rescue the
provider: the stored key shadows the ambient env forever and the only exit is `/logout`. The vertex
port has the same fallbacks correctly (`providers/google_vertex.rs:177-181`, `:200-223`); cloudflare
is the outlier.

### The remaining work, measured

Upstream's four bodies total **101 lines of TypeScript** (5 + 10 + 47 + 39). Everything they need on
the cyrup side is already ported and already exercised by the OAuth flows:

- `AuthPrompt::secret` / `::text` / `::select` with `AuthSelectOption`
  (`crates/cyrup-provider/src/auth/oauth/interaction.rs:16-80`);
- `AuthEvent::Info { message, links }` and `AuthInfoLink` (`interaction.rs:103-107`), rendered by
  `LoginDialog::show_info` including link rows (`crates/cyrup-tui/src/login_dialog.rs:284-292`);
- in-dialog `select` rendering (`login_dialog.rs:266-272`), already driven by the Codex flow's own
  three-way picker;
- `Credential::ApiKey { key: Option, env: Option<BTreeMap> }` — key-less and env-carrying credentials
  both serialize (`crates/cyrup-config/src/auth.rs:21-35`);
- exact-message error passthrough for pi's `throw new Error("Unknown Google Vertex AI auth method: …")`
  via `OAuthError::Failed(String)` (`auth/oauth/mod.rs:104-107`) → `LoginError::Flow`'s transparent
  `Display` (`login.rs:169-174`).

The only structural change is one trait member — and **that member is already scheduled**:
`PROV-003`'s Fix (`01-cyrup-core-and-provider.md:191`) adds `login` to `ApiKeyAuth` in batch 11. The
marginal cost of `CFG-005` on top of `PROV-003` is four function bodies, a one-line call-site swap,
and the deletion of a name-sniffing predicate. `CFG-005`'s recorded effort of **L** was sized when
the whole OAuth surface was missing and is stale by an order of magnitude.

## Decision

**Schedule all four flow bodies into batch 11, in the same diff as `PROV-003`.** Do not ship
google-vertex alone; do not hold cloudflare; do not carry the deprioritisation forward. Concretely,
an implementer does this and re-derives nothing:

1. **Add the trait member** (this is `PROV-003`'s edit; `CFG-005` consumes it):
   ```rust
   // crates/cyrup-provider/src/auth/mod.rs, in `trait ApiKeyAuth` (currently :59-71)
   async fn login(&self, _interaction: &dyn oauth::AuthInteraction)
       -> Result<Credential, oauth::OAuthError> {
       Err(oauth::OAuthError::LoginUnsupported { name: self.name().to_string() })
   }
   fn supports_login(&self) -> bool { false }
   ```
   Return `OAuthError`, **not** `AuthError`, so `LoginError::Flow`'s message passthrough
   (`login.rs:169-174`) and the dialog's `"Login cancelled"` comparison keep working unchanged. The
   `supports_login` companion is what pi's `method.login !== undefined`
   (`interactive-mode.ts:4940`) means in a language with no optional members; every impl that
   overrides `login` overrides it to `true`.

2. **Delete the name sniffer.** Remove `api_key_strategy_supports_login` (`login.rs:316-318`) and
   its two call sites (`:476`, `:793`); read `s.supports_login()` instead. `env_key` and each of the
   four bespoke strategies return `true`; `keyless_local` returns `false` (preserving today's only
   correct answer, `login.rs:1061`'s ambient-dialog assertion). `env_api_key_login`
   (`login.rs:343-351`) becomes `EnvKeyAuth::login`'s body — keep the free function, keep the
   `Enter {name}` label reconstruction at `login.rs:327-333`, and keep the prompt text byte-identical
   to `helpers.ts:13`.

3. **Port the four bodies verbatim**, prompt for prompt and in upstream's order:
   - `CloudflareWorkersAiAuth::login` ← `cloudflare-auth.ts:50-54`: secret `"Enter Cloudflare API key"`,
     text `"Enter Cloudflare account ID"` → `{key, env:{CLOUDFLARE_ACCOUNT_ID}}`.
   - `CloudflareAiGatewayAuth::login` ← `cloudflare-auth.ts:70-79`: the same two, then text
     `"Enter Cloudflare AI Gateway ID"` → `{key, env:{CLOUDFLARE_ACCOUNT_ID, CLOUDFLARE_GATEWAY_ID}}`.
   - `GoogleVertexApiKeyAuth::login` ← `google-vertex.ts:15-61`: select
     `"Select Google Vertex AI authentication method:"` over ids `api-key` / `adc` /
     `service-account`; `api-key` → secret → `{key}`; any other id that is neither `adc` nor
     `service-account` → `OAuthError::Failed(format!("Unknown Google Vertex AI auth method: {method}"))`;
     otherwise `notify(AuthEvent::Info)` with the method-dependent message at `:36-39` and the ADC
     doc link at `:41-44`, then text project, text location, and for `service-account` a third text
     `"Enter service account credentials file path"` → `{env:{GOOGLE_CLOUD_PROJECT,
     GOOGLE_CLOUD_LOCATION[, GOOGLE_APPLICATION_CREDENTIALS]}}` with **no** key.
   - `AmazonBedrockApiKeyAuth::login` ← `amazon-bedrock.ts:13-51`: select over `bearer-token` /
     `aws-profile` / `credential-chain`; bearer → secret → `{key}`; otherwise `notify` with the AWS
     credential-provider-chain link at `:29-38`; `aws-profile` → text → `{env:{AWS_PROFILE}}`;
     `credential-chain` → the text prompt `"Configure AWS credentials, then press Enter to continue"`
     used purely as a barrier → `{}` (key-less, env-less); an unknown id →
     `OAuthError::Failed(format!("Unknown Amazon Bedrock auth method: {method}"))`, matching `:45`
     byte for byte (upstream's select message at `:16` says "authentication method"; its throw at
     `:45` says "auth method" — reproduce both as written, do not harmonise them).

4. **Fix cloudflare's ambient fallback in the same diff.** Restore pi's `?? ctx.env(name)` at
   `crates/cyrup-provider/src/providers/cloudflare.rs:48-65` so a credential missing a field falls
   through to the environment. Without it, step 3 still leaves every pre-existing key-only cloudflare
   credential permanently unusable, and the item would close while the user-visible symptom
   persisted.

5. **Rewrite the four in-source notes that assert the hole**, in the same diff, or batch 3's
   unwired-claim review will re-file it: `crates/cyrup-config/src/login.rs:31-42` and `:309-317`,
   `crates/cyrup-provider/src/providers/google_vertex.rs:37-43`, and
   `crates/cyrup-provider/src/providers/amazon_bedrock.rs:65-72`.

6. **Verification (all offline; no provider API is contacted).** Drive each of the four through
   `cyrup_config::login::login` with a `ScriptedInteraction`, then assert the *stored* credential is
   consumed by that provider's own `resolve` with an empty `AuthContext`: workers-ai and ai-gateway
   yield a substituted `baseUrl` plus the env overlay; bedrock's `aws-profile` arm resolves through
   the `AWS_PROFILE` rung with `auth: {}`; vertex's `adc` arm resolves through the ADC rung with
   `auth: {}` given a stubbed `file_exists`. Add a picker test that `keyless_local` is still the only
   strategy routed to `LoginStep::Ambient`, and a cloudflare test that a key-only credential plus an
   ambient `CLOUDFLARE_ACCOUNT_ID` resolves (step 4's regression guard).

**Rationale under the standing rule.** The scope narrowed exactly as the plan suspected, but in the
other direction from what was recorded: it is four bodies, not two, and the symptom is a wrong
answer, not an absence. A deprioritisation granted against "a subsystem is missing" does not survive
the discovery that the subsystem is present and mis-wired. No impossibility and no project
constraint is in tension here — the substrate is ported, the seam lands in batch 11 anyway, and the
remaining cost is ~101 lines of behaviour.

## Consequences

**Ledger.** `CFG-005` (`05-cyrup-config-and-resources.md:53`, `:134`, `:420-432`) changes on four
axes: **severity** medium → **high** (a control that advertises a login, reports success, and stores
an unusable credential is a wrong answer, not a missing one, and it hits four registered providers);
**kind** `not-ported` → **`parity-bug`**; **effort** L → **M**; **scope** widened from two flows to
four bodies across four provider ids, adding `cloudflare-ai-gateway` and `amazon-bedrock` and naming
`login.rs:316-318`'s predicate as the defect's proximate cause. Its "**Maintainer has DEPRIORITISED
this item: keep filed, do not schedule**" line (`:432`) is **withdrawn** by this ADR, as is the
`00-residual-ledger.md:379-383` "Still genuinely deferred, by decision" bullet.

`PROV-003` (`01-cyrup-core-and-provider.md:175-201`) keeps its medium/M rating but its **Fix
(`:191`) is amended on three points**: the signature becomes `Result<Credential, OAuthError>`, not
`Result<Credential, AuthError>` as written there — `AuthError` would force a new `LoginError` arm and
break the flow-message passthrough `login.rs:169-174` depends on; the new member must be *consumed*
at `login.rs:786-802`; and `api_key_strategy_supports_login` must be *deleted* in the same diff.
Landing the member without the last two leaves the wrong flow running and would close `PROV-003` on
a defect it caused. Its
"anthropic" clause is unaffected — cyrup's anthropic uses `env_key`, so
`api_key_login_label` already reproduces `Enter Anthropic API key` byte-identically
(`login.rs:327-333`); if `PROV-021` replaces it with a bespoke strategy named `"Anthropic API key"`,
that strategy needs a one-line `login` that calls `env_api_key_login`.

**Batch 11.** Gains `CFG-005` — a `cyrup-config` + `cyrup-provider` item in a batch already opened by
the OAuth/bedrock/codex closure audit and already holding `PROV-003` and `PROV-029`. It is the same
reviewer reading the same files; the batch's stated risk (largest single-crate batch before the WIT
bump) is unchanged by four small bodies, and if the batch splits at the audit boundary, `CFG-005`
travels with `PROV-003`, never apart from it. Note for the batch author: `PROV-029` is one field
assignment per provider in `providers/builtin_oauth.rs:37` and is **not** this item — do not merge
the two edits or the `lint-unwired` acceptance signal for `PROV-029` becomes unreadable.

**Batch 12.** `PROV-030`'s google-vertex end-to-end step no longer needs a hand-provisioned
`auth.json` entry: `/login google-vertex` → *Application Default Credentials* → project → location
writes the credential the wire port needs. Stated precisely so nobody over-claims: the user must
still have run `gcloud auth application-default login` for the ADC file to exist on disk — the ported
flow is what *tells* them to (`google-vertex.ts:36-39`) and what stores the project/location the
`{location}` base-URL template interpolates. The hand-edit disappears; the gcloud step does not.

**New work this ADR creates that no item covers.** The cloudflare `resolveValue` ambient-env
fallback (`providers/cloudflare.rs:48-65` vs `cloudflare-auth.ts:18-23`) is a confirmed parity bug
appearing in no gap-analysis row. It belongs to area 01 (`01-cyrup-core-and-provider.md` owns
`providers/cloudflare.rs`) and needs a new `PROV-*` id assigned by that file's owner; this ADR does
not assign one, and step 4 above lands the fix in batch 11 regardless of where it is filed.

**Explicitly out of scope, so the omission is not silent.** `ApiKeyAuth.check?`
(`pi/packages/ai/src/auth/types.ts:173`) is also unported, but no built-in provider defines it at
v0.83.0 — its only consumer is `Models.checkProviderAuth` (`pi/packages/ai/src/models.ts:364-391`),
so it belongs to `PROV-031`'s `Models` surface, not to `CFG-005`. The `~163 lows` half of OQ-8 is
untouched by this ADR.

## Rejected alternatives

**Hold the deprioritisation.** Rejected. It rests on a description of the world that is no longer
true in either direction: the subsystem it deferred has landed, and the residual is not the passive
gap the deferral assumed. Holding it means shipping a `/login` that offers four providers and,
for the two cloudflare ids, cannot possibly succeed — with no error, and with a stored credential
that then shadows the working env-var path (step 4's defect). Cost of taking it: four registered
providers keep a login control that lies, `amazon-bedrock` stays unfiled entirely, and batch 12
keeps a manual precondition on its verification.

**Ship google-vertex only, hold cloudflare.** Rejected. It has the ordering exactly backwards.
Vertex is the one provider of the four whose accidental single-secret flow *coincidentally works*
(its api-key rung takes a bare key, `providers/google_vertex.rs:177-198`), while cloudflare is the
one where the current behaviour is a guaranteed dead end. Porting vertex first fixes the least
broken case and leaves the most broken one shipping. It also saves nothing: the trait member,
the predicate deletion and the call-site swap — the whole structural cost — are paid by the first
body, whichever it is; each additional body is 5-45 lines. Cost of taking it: two cloudflare
providers stay broken for a batch-or-more, for zero saved structural work.

**Add the four bodies but keep `api_key_strategy_supports_login` as the predicate.** Rejected. The
sniffer works today only because no shipped strategy is both bespoke and login-less; it is a
coincidence with no upstream analogue, and it silently mis-answers the moment an extension or a new
provider supplies an ambient-only strategy with a display name (which is exactly what pi's
`ApiKeyAuth` docs invite: *"Ambient-only providers omit `login`"*, `auth/types.ts:157-159`). Cost of
taking it: the same lying-control failure returns on the next provider, undetectably.

**Make the predicate answer `false` for the four (advertise nothing until the flows exist).**
Rejected as an interim, not as a bad instinct — it would at least stop the silent mis-store, routing
the four to the honest ambient dialog (`"Cloudflare API key is configured outside pi."`). But it is
a divergence with no upstream analogue, and under the no-accepted-divergence rule the behavioural
cost would stay on the backlog anyway. It is worth landing **only** if batch 11 splits and `CFG-005`
misses the split — three lines, reverted by step 2 when the flows land. Cost of taking it as the
final answer: four providers permanently lose interactive setup that pi has.

## How to reverse this

*"Cloudflare, Bedrock and Vertex are not providers we support interactively — leave the flows
unported and make `/login` say so."* — for that to hold, `providers/all.rs:167-190` must stop
registering the providers whose login is unported (a registered-but-unloggable provider is the
`PROV-030` failure mode again), the four rows must be routed to `LoginStep::Ambient` by a real
predicate rather than a name sniffer, and the behavioural cost must be written into `CFG-005` as an
accepted divergence — which today's standing rule does not have a category for, so reversing this
ADR also requires amending that rule.
