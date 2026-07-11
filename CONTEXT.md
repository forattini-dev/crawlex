# crawlex

Stealth-first web crawler. Workspace orchestrates HTTP impersonation, Chrome rendering, link discovery, antibot handling, and persistent queues behind a single `Crawler` orchestrator.

## Language

### Crawl orchestration

**Job**:
One unit of work: a URL plus depth, priority, fetch method, and retry state.
_Avoid_: task, request, work item

**Method**:
Fetch strategy chosen for a Job: `spoof`, `render`, or `auto`.
_Avoid_: mode, backend

**Crawler**:
Top-level orchestrator. Owns the run loop, admission decisions, budget accounting, storage writes, and run-level lifecycle. Does not execute a Job itself.
_Avoid_: engine, worker

**JobRunner**:
Executes one Job against a SessionContext and produces a JobOutcome. Pure of queue, storage, admission, and frontier concerns.
_Avoid_: job executor, processor

**JobOutcome**:
Structured result of one Job: response, extracted links, challenge signals, timings, retry decision. Returned by value; no side-effect channels.
_Avoid_: result, response

**SessionContext**:
Per-attempt inputs to a JobRunner: identity, proxy lease, antibot session state, budgets, resolved policy profile. Built by the Crawler.
_Avoid_: request context, session

**Fetcher**:
Trait that turns a Job + SessionContext into a FetchResponse. Variants: `SpoofFetcher`, `RenderFetcher`, `AutoFetcher` (composes the other two with an escalation decider).
_Avoid_: client, downloader

**Extractor**:
Pure component that derives links and asset classifications from a FetchResponse. Concrete struct, no trait.

**ChallengeDetector**:
Pure component that inspects a FetchResponse and emits a `ChallengeSignal` when an antibot interstitial is recognised. *Superseded by [[Fingerprinter]] Hot tier (ADR-0003) — kept here for historical reference until the substitution lands.*

**Fetcher::fetch**:
Returns a `FetchOutput` (enum: `Http(impersonate::Response)` | `Rendered(RenderedPage)`). Common helpers (`status`, `headers`, `body`, `final_url`) on the enum let extractors and detectors stay variant-agnostic.

### Fingerprint (target + self)

**Fingerprinter**:
Engine that consumes a TargetContext and emits a FingerprintReport. Holds a registry of Sources, partitioned by Tier (Hot / Warm / Cold). Replaces `discovery::tech_fingerprint`, `runner::ChallengeDetector`, and the detect-* fns in `antibot::*` (action modules — bypass, cookie_pin, solver, telemetry, recaptcha — stay where they are).
_Avoid_: TechFingerprinter, Detector, Analyzer

**Detection**:
One finding emitted by a Source. Carries `{ category: Category, vendor: String, version: Option<String>, confidence: Confidence, evidence: Vec<Evidence> }`. The unit the Fingerprinter aggregates into the report.

**Evidence**:
Why a Detection fired. `{ source: EvidenceSource, detail: String, weight: u8 }`. `source` is the kind of signal (Header, CookieName, BodyMarker, TlsServerHello, …); `detail` is human-readable proof; `weight` 1–10 feeds the Confidence bucket via a fixed rule.

**Confidence**:
`High | Medium | Low`. Derived from summed evidence weights — never set directly. Threshold rule lives in `fingerprint/detection.rs`; tested in isolation.

**Vendor**:
Consolidated identity enum — Cloudflare, Akamai, Fastly, DataDome, PerimeterX, Imperva, etc. Replaces three legacy enums: `error::AntibotVendor`, `antibot::ChallengeVendor`, the `vendor` field on `runner::ChallengeSignal`. Old types kept as `#[deprecated]` re-exports for one release.

**Category**:
What kind of thing the Vendor is. CDN, WAF, Antibot, CMS, Ecommerce, FrontendFramework, BackendRuntime, WebServer, ReverseProxy, Cache, Analytics, TagManager, AbTesting, Auth, Payment, Chat, DnsHosting, TlsProfile, HttpFingerprint, CookiePattern, Other. Extensible — non-exhaustive enum.

**Source**:
Trait that produces Detections from a TargetContext. `{ name(): &'static str, tier(): Tier, analyze(&TargetContext) -> Vec<Detection> }`. 20+ initial impls (Header, CookieName, BodyMarker, MetaTag, ScriptSrc, LinkRel, JsonLd, TlsServerHello, AltSvc, PeerCert, StatusPattern, TimingPattern, H2Settings, RobotsTxt, WellKnown, FaviconHash, Dns, Asn, AntibotMarker, BlockPattern). New sources added without touching the engine.

**Tier**:
When a Source runs. `Hot` (free, every fetch — runs on `analyze_hot`), `Warm` (medium cost, per-host once, cached — runs on `analyze_warm`), `Cold` (external network probes, opt-in via `--deep-fingerprint` — runs on `analyze_cold`).

**TargetContext**:
Input bundle the Fingerprinter passes to each Source. Carries status, headers, body, TLS observation, final URL, and `Option<&T>` slots for warm/cold-only data (h2 settings, robots.txt, well-known, favicon hash, DNS observation, ASN, peer cert).

**FingerprintReport**:
Aggregated output. One struct per host, typed slots per Category (`cdn: Vec<Detection>`, `waf: Vec<Detection>`, `antibot: Vec<Detection>`, …) plus single-detection slots (`tls_profile: Option<TlsProfile>`, `http_fingerprint: Option<HttpFp>`). Includes `tiers_run` bitflags, `stale_at` cache TTL, embedded `self_fp: Option<SelfFingerprint>`, and a `coherence: Coherence` cross-check.

**Coherence**:
Cross-check between FP-A (target) and FP-B (self). Surfaces "our JA3 matches our Profile expectation" and "our profile is plausible against the antibot vendor we detected" — the diff vs. plain tech-detection (redblue baseline).

**SelfFingerprint**:
What we expose outbound: JA3, JA4, JA3 hash, h2 SETTINGS fingerprint, observed header order, sec-ch-ua tier. Built by `fingerprint/self/` from three sources — live capture of our own ClientHello bytes (truth), static catalog keyed on Profile (expected baseline, ADR), external oracle (opt-in audit, `--audit-tls`).

### Policy and identity

**PolicyProfile**:
Resolved set of budgets and thresholds: `fast`, `balanced`, `deep`, `forensics`.

**SessionIdentity**:
Per-session browser persona: HTTP profile, cookie jar, persona mutations. Today threaded as `ImpersonateClient + IdentityBundle + cookies`; will be unified.
_Avoid_: identity, profile (alone)

**SessionState**:
Antibot finite-state snapshot for one session. Mutated only via JobOutcome; never in-place from within a JobRunner.

**StealthProfile**:
Durable per-host stealth memory in the RedDB index plane: the detected antibot Vendor, the SelfFingerprint / Profile that last passed, cookies / session state that survived (via StateStorage), the observed crawl-delay, the coherent proxy lease, and challenge / ban history. Loaded and applied when the Crawler builds a SessionContext for a Job to that host (priming), so a known-good identity is reused instead of cold-started. Reuse/rotate follows the `SessionState` ladder (coherence-aware, ADR-0008): `Clean`/`Warm` reused, terminal `Blocked` (hard block) burned, `Contaminated` (solved/recovered challenge) kept-but-derated and rotated only after K; a solved-challenge-then-200 does not burn. Rotation is rate-limited per host/IP to avoid a churn cluster signal.
_Avoid_: session (per-attempt), SessionIdentity (the live persona), host_facts (write-side facts only)

**CadenceGovernor**:
The stealth dimension that shapes request timing to look human, not merely polite: per-page ReadingDwell, inter-request jitter, session length + breaks, optional circadian pacing, per host / identity. Observed cadence is written to the `crawlex_crawl_events` time-series and self-monitored (burstiness / frequency thresholds) so the crawler backs off when its own pattern drifts toward bot-like. Bounded below by the HostRateLimiter (politeness floor) and scaled by the PolicyProfile (`fast` disables shaping, `forensics` maximises it). ADR-0009.
_Avoid_: rate limiter (politeness only), throttle

**Challenge memory**:
A RedDB vector collection of challenge / interstitial page embeddings. On a fetch that no static `antibot::signatures` entry matches, a `SEARCH SIMILAR` hit against it flags a probable antibot challenge — a learned Source feeding the Fingerprinter (category Antibot) alongside the static table; confirmed challenges are embedded back so mutated interstitials are recognised per Vendor. Embeds only challenge / block pages, not every page. Inactive (static signatures still run) when no RedDB embedding provider is configured; a hit is never sole-High Confidence. ADR-0011.
_Avoid_: challenge telemetry (the session-keyed replay log)

### Frontier and persistence

**Frontier**:
The set of URLs admitted but not yet crawled, held in a durable queue — RedDB stream `crawlex_frontier` + job table + dead-letter `crawlex_dead_letter`, or an in-memory / SQLite backend. Survives restarts, so a Crawl resumes from its pending Jobs.
_Avoid_: queue (alone), backlog

**Crawl**:
A first-class persisted run, registered in the RedDB `crawls` collection by a stable `crawl_id` with its seeds, resolved PolicyProfile, scope (one-shot depth-0 vs recursive full + max depth), status (`running | paused | done | failed`), and timestamps. One-shot and full crawl are the same Crawl differing only in scope. `crawl resume <id>` reopens its Frontier + Seen-set and continues; `crawl refresh <id>` re-seeds it under the Seen-set TTL (ADR-0005, ADR-0007). A Crawl may span many executions — each execution is one run with its own `run_id`; resuming starts a new run over the same `crawl_id`.
_Avoid_: run (one execution of a Crawl), job (one URL within a Crawl)

**Host-eligibility scheduling**:
How the Frontier chooses the next Job: the highest-priority Job among hosts whose `next_allowed_at` has arrived, where `next_allowed_at = max(HostRateLimiter rps floor, robots crawl-delay, CadenceGovernor envelope)`. Interleaves eligible hosts (round-robin, priority as within-host order) to keep workers busy without same-domain thrash; per-host schedule state (`next_allowed_at`, `last_crawled`) is persisted in RedDB so resume and refresh preserve pacing. Same-host preference is a tiebreak within the eligible set, not a global bias. ADR-0010.
_Avoid_: priority queue (alone), FIFO

**Seen-set**:
The authoritative record of which URL identities have already been admitted, so a URL is not re-enqueued. Backed by a durable KV; each entry is scoped by last-crawled-time and re-admitted once older than a TTL (ADR-0005), never a bare permanent flag. The bloom + bounded exact-recent set in `frontier::Dedupe` is a non-authoritative L1 speed hint over it.
_Avoid_: visited-set, dedup cache

**Content identity**:
A page's dedup key derived from its normalised text (post `extract::html_clean`), distinct from its URL identity — robust to CSRF tokens, timestamps, and session ids that make raw-HTML hashes differ trivially. Two URLs that share a Content identity share one stored body; the second URL and its graph edge are still recorded, so discovery is never lost (non-destructive dedup, ADR-0006). Also the same-URL refresh short-circuit: unchanged content identity on a TTL re-crawl bumps `last_crawled` without a re-store.
_Avoid_: content hash (alone), fingerprint (reserved for the target-stack term)

**BlobStore**:
The byte plane — a content-addressed store for page bodies and artifacts, keyed by `cas:<content-hash>` (the Content identity). Pluggable backend: `fs` (default, zero-config), `s3`, `r2` (ADR-0006). Distinct from the index/memory plane (RedDB), which holds only the pointer + metadata; RedDB never stores large bodies inline, matching its own reference-by-URI doctrine. Writes are bytes-first, pointer-second.
_Avoid_: object store (alone), cache (that is an internal RedDB concern)

## Relationships

- A **Crawler** runs many **Jobs** through one or more **JobRunners**.
- A **JobRunner** consumes a **Job** + **SessionContext** → produces a **JobOutcome**.
- A **JobRunner** delegates fetching to a **Fetcher**, link extraction to an **Extractor**, and detection to a **Fingerprinter** (Hot tier).
- A **Fetcher** variant is chosen by the **Method** on the **Job**.
- A **Fetcher::fetch** returns a **FetchOutput** carrying either an HTTP response or a rendered page.
- A **SessionContext** carries one **SessionIdentity**, one optional proxy lease, and one **SessionState**.
- A **Fingerprinter** holds Sources partitioned by **Tier**. Hot tier runs every fetch; Warm tier runs once per host (cached); Cold tier runs on-demand.
- A **Source** consumes a **TargetContext** and emits zero or more **Detection**s.
- A **Detection** belongs to one **Category** and carries one **Vendor** + one **Confidence** + a `Vec<`**Evidence**`>`.
- A **FingerprintReport** aggregates Detections per host, embeds the run's **SelfFingerprint**, and exposes a **Coherence** cross-check.

## Example dialogue

> **Dev:** "When a **Job** comes off the queue, who decides whether to **spoof** or **render**?"
> **Domain expert:** "The **Crawler** picks the **Method** at admission. The **AutoFetcher** can still escalate spoof→render mid-run if the **ChallengeDetector** fires."

> **Dev:** "If antibot state changes during a **Job**, does the **JobRunner** mutate the **SessionState** directly?"
> **Domain expert:** "No. The **JobRunner** returns the new **SessionState** inside the **JobOutcome**. The **Crawler** commits it. Runners stay value-in/value-out."

## Flagged ambiguities

- "session" was used to mean both **SessionContext** (per-attempt) and **SessionIdentity** (persona that survives many Jobs) — resolved: distinct.
- "auto" historically named both a **Method** and the **AutoFetcher** type — resolved: `auto` is the Method; **AutoFetcher** is the impl.
- "ChallengeSignal" used to live in three places (`antibot::ChallengeSignal`, `runner::ChallengeSignal`, plus an implicit shape inside `error::AntibotChallenge`) — resolved (ADR-0003): one **Detection** with `category: Antibot` is the single representation; the three legacy types collapse into the consolidated **Vendor** enum.
- "fingerprint" used to mean either the tech-stack of the target (`discovery::tech_fingerprint`) or our own outbound identity (loosely in `identity::*`) — resolved: **TargetContext**/**FingerprintReport** for FP-A (their stack), **SelfFingerprint** for FP-B (our outbound). Both live under `fingerprint/`.
- "crawl" / "run" were used interchangeably — resolved (ADR-0007): a **Crawl** is the durable persisted entity (`crawl_id`); a run is one execution of it (`run_id`). Resuming a Crawl opens a new run over the same `crawl_id`.
