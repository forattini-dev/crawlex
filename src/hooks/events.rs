#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookEvent {
    BeforeEachRequest,
    AfterDnsResolve,
    AfterTlsHandshake,
    AfterFirstByte,
    OnResponseBody,
    AfterLoad,
    AfterIdle,
    OnDiscovery,
    OnJobStart,
    OnJobEnd,
    OnError,
    OnRobotsDecision,
}
