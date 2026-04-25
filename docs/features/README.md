# Features Overview

The crawler is built around a few capability clusters instead of dozens of disconnected subcommands.

## Capability groups

- [Discovery Enrichment](/features/discovery.md): probes that widen the frontier around each seed.
- [Proxy and Stealth](/features/proxy-stealth.md): identity coherence, rotation strategies and fingerprint inspection.
- [Storage and Outputs](/features/storage-outputs.md): durable run state, artifacts and exports.

## Default posture

The default posture is conservative:

- HTTP-first
- robots respected
- cookies enabled
- redirects followed
- render pool disabled unless requested
- expensive metrics and artifacts off unless requested
