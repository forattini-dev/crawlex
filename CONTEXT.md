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
Pure component that inspects a FetchResponse and emits a `ChallengeSignal` when an antibot interstitial is recognised.

### Policy and identity

**PolicyProfile**:
Resolved set of budgets and thresholds: `fast`, `balanced`, `deep`, `forensics`.

**SessionIdentity**:
Per-session browser persona: HTTP profile, cookie jar, persona mutations. Today threaded as `ImpersonateClient + IdentityBundle + cookies`; will be unified.
_Avoid_: identity, profile (alone)

**SessionState**:
Antibot finite-state snapshot for one session. Mutated only via JobOutcome; never in-place from within a JobRunner.

## Relationships

- A **Crawler** runs many **Jobs** through one or more **JobRunners**.
- A **JobRunner** consumes a **Job** + **SessionContext** → produces a **JobOutcome**.
- A **JobRunner** delegates fetching to a **Fetcher**, link extraction to an **Extractor**, and challenge detection to a **ChallengeDetector**.
- A **Fetcher** variant is chosen by the **Method** on the **Job**.
- A **SessionContext** carries one **SessionIdentity**, one optional proxy lease, and one **SessionState**.

## Example dialogue

> **Dev:** "When a **Job** comes off the queue, who decides whether to **spoof** or **render**?"
> **Domain expert:** "The **Crawler** picks the **Method** at admission. The **AutoFetcher** can still escalate spoof→render mid-run if the **ChallengeDetector** fires."

> **Dev:** "If antibot state changes during a **Job**, does the **JobRunner** mutate the **SessionState** directly?"
> **Domain expert:** "No. The **JobRunner** returns the new **SessionState** inside the **JobOutcome**. The **Crawler** commits it. Runners stay value-in/value-out."

## Flagged ambiguities

- "session" was used to mean both **SessionContext** (per-attempt) and **SessionIdentity** (persona that survives many Jobs) — resolved: distinct.
- "auto" historically named both a **Method** and the **AutoFetcher** type — resolved: `auto` is the Method; **AutoFetcher** is the impl.
