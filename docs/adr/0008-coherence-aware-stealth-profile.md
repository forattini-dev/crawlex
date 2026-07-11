# Coherence-aware durable StealthProfile per host

`crawlex` has the machinery to persist stealth state but does not turn it into reusable memory:

- `StateStorage::save_state` / `load_state` can persist a session's opaque state (cookies + storage + service-worker registrations) keyed by `session_id`, described as resuming a session across runs — but the trait defaults are no-ops, so unless a concrete backend implements them it is off.
- `SessionRegistry` (the live identity lifecycle: TTL, challenge count, bundle, proxy) is an in-memory `DashMap`. It vanishes on restart.
- `host_facts` is an in-memory `DashMap`; `save_host_facts` persists it, but there is no read-back — nothing re-primes a fresh crawl from stored facts. It is write-mostly memory: the crawler re-learns each host's antibot vendor, DNS, and manifest every run.
- Nothing primes antibot knowledge before a fetch. `antibot::signatures` is a static table; there is no per-host learned memory of the form "this host is DataDome and Profile X passed last time".

So every run cold-starts identity and re-learns every host — wasteful, and it makes the crawler present a brand-new, un-warmed persona each time (it "is born suspicious").

The bot-detection reference sharpens the constraint. Defenders cluster on network and behavioural similarity and score coherence, so **reusing a burned identity increases detectability** — if a host has already linked a prior session to a challenge or ban, re-presenting that cookie set / fingerprint is worse than starting fresh. Durable stealth memory must therefore be coherence-aware, not blind reuse.

Decision: a durable **StealthProfile** per host, in the RedDB index plane, holding the detected antibot `Vendor`, the `SelfFingerprint` / Profile that last passed, the cookies / `session_id` that survived (via `StateStorage`), the observed crawl-delay / politeness, and the challenge / ban history. When the Crawler builds a `SessionContext` for a Job to that host, it **loads the StealthProfile and primes the session with the last known-good identity** — reusing a warm, working persona instead of cold-starting — **but rotates and burns it (fresh cold-start) when the last recorded outcome for that host + identity was a challenge or ban**. This folds the ephemeral `SessionRegistry` snapshot, the write-mostly `host_facts`, and the dormant `StateStorage` into one durable, re-primed unit, and it uses the existing `Coherence` cross-check as the reuse/rotate gate.

Consequences and open points:

- **Read-back is now required.** `load_host_facts` (and a `load_stealth_profile`) must exist and run at admission / session-build; a concrete `StateStorage` backend must be wired (the no-op default cannot be the only impl).
- **Burn policy (resolved).** The durable burn maps onto the `SessionState` ladder the runtime already computes — `Clean → Warm → Contaminated → Blocked`, monotonic, with `Blocked` sticky (`(_, HardBlock) => Blocked`, "a Blocked session never downgrades to Clean"). An identity that ended `Clean` / `Warm` is reused; a terminal `Blocked` (hard block) **burns** it — rotate + cold-start next time for that host; a `Contaminated` state (a soft challenge that was solved / recovered) is **kept but has its trust decremented**, rotating only after `K` consecutive `Contaminated` outcomes. A solved-challenge-then-200 does not burn (proven-good identity). Rotation is itself **rate-limited per host / IP** so identity churn does not become the network-cluster signal the bot-detection reference scores. Only `K` and the rotation-rate cap remain as tuning constants.
- **Proxy / IP coherence.** IP is part of what the defender clusters on, so a reused identity should prefer the proxy lease that worked with it; rotating identity after a burn should also rotate the IP. The StealthProfile therefore references the proxy that was coherent with the persona, not just the cookies.
- **Blast radius.** Reusing cookies links crawls of the same host together — acceptable within one operator — and the profile is per-host, so one host's burn does not cross-contaminate other hosts.

Alternatives rejected: persist-but-cold-start by default (safe, but leaves the re-learn waste and the born-suspicious problem unsolved); keep write-mostly (no reuse benefit at all).
