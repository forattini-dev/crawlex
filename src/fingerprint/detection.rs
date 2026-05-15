//! Shared detection types — `Detection`, `Evidence`, `Confidence`,
//! `Vendor`, `Category`, `Tier`.
//!
//! Slice B1 of PRD forattini-dev/crawlex#25. These types are the unit
//! of work every Source emits and every consumer (runner, CLI, reports)
//! consumes. Confidence is **derived** deterministically from summed
//! evidence weights — never set directly. The threshold rule lives in
//! [`Confidence::from_evidence`].

use serde::{Deserialize, Serialize};

/// One finding emitted by a Source. Carries enough context for an
/// operator to act on it without further investigation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Detection {
    pub category: Category,
    pub vendor: Vendor,
    pub version: Option<String>,
    pub confidence: Confidence,
    pub evidence: Vec<Evidence>,
}

impl Detection {
    /// Build a Detection from a single Evidence. Confidence derived
    /// from the single weight via [`Confidence::from_evidence`].
    pub fn from_single(category: Category, vendor: Vendor, evidence: Evidence) -> Self {
        let confidence = Confidence::from_evidence(std::slice::from_ref(&evidence));
        Self {
            category,
            vendor,
            version: None,
            confidence,
            evidence: vec![evidence],
        }
    }

    /// Build a Detection from a list of Evidences. Confidence derived
    /// from the summed weights.
    pub fn from_evidence(category: Category, vendor: Vendor, evidence: Vec<Evidence>) -> Self {
        let confidence = Confidence::from_evidence(&evidence);
        Self {
            category,
            vendor,
            version: None,
            confidence,
            evidence,
        }
    }
}

/// Why a Detection fired. Source + human-readable proof + weight.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub source: EvidenceSource,
    pub detail: String,
    pub weight: u8,
}

impl Evidence {
    pub fn new(source: EvidenceSource, detail: impl Into<String>, weight: u8) -> Self {
        Self {
            source,
            detail: detail.into(),
            weight: weight.clamp(1, 10),
        }
    }
}

/// Kind of signal that produced an Evidence. Mirrors the Source list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSource {
    Header,
    CookieName,
    BodyMarker,
    JsonLd,
    MetaTag,
    ScriptSrc,
    LinkRel,
    TlsServerHello,
    H2Settings,
    AltSvc,
    Dns,
    Asn,
    PeerCert,
    RobotsTxt,
    WellKnown,
    FaviconHash,
    StatusPattern,
    TimingPattern,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

impl Confidence {
    /// Threshold rule:
    ///   - any evidence weight 10 → High
    ///   - summed weight >= 10 → High
    ///   - summed weight >= 6 OR 2+ evidences of weight >= 4 → Medium
    ///   - otherwise → Low
    ///
    /// Constants kept here so reviewers see the rule next to the type.
    pub fn from_evidence(evidence: &[Evidence]) -> Self {
        if evidence.is_empty() {
            return Confidence::Low;
        }
        if evidence.iter().any(|e| e.weight >= 10) {
            return Confidence::High;
        }
        let sum: u32 = evidence.iter().map(|e| e.weight as u32).sum();
        if sum >= 10 {
            return Confidence::High;
        }
        let strong_count = evidence.iter().filter(|e| e.weight >= 4).count();
        if sum >= 6 || strong_count >= 2 {
            return Confidence::Medium;
        }
        Confidence::Low
    }
}

/// Consolidated vendor identity. Replaces `error::AntibotVendor`,
/// `antibot::ChallengeVendor`, and the `vendor` field on
/// `runner::ChallengeSignal` (collapse completes in B7). Non-exhaustive
/// so new vendors land without breaking downstream.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Vendor {
    // CDN / Edge
    Cloudflare,
    Akamai,
    Fastly,
    CloudFront,
    Bunny,
    Vercel,
    Netlify,

    // WAF
    AwsWaf,
    ImpervaWaf,
    SucuriWaf,
    F5BigIp,
    Wallarm,
    ModSecurity,

    // Antibot / Bot management
    DataDome,
    PerimeterX,
    Imperva,
    DistilNetworks,
    Kasada,
    ShapeSecurity,
    AkamaiBotManager,
    CloudflareBotManagement,
    CloudflareTurnstile,
    HCaptcha,
    Recaptcha,

    // CMS / Platform
    Wordpress,
    Drupal,
    Joomla,
    Ghost,
    Wagtail,
    Magento,
    Shopify,
    Vtex,
    BigCommerce,
    SalesforceCommerce,
    WooCommerce,

    // Server / Proxy
    Nginx,
    Apache,
    Iis,
    Caddy,
    LiteSpeed,
    OpenResty,
    Haproxy,
    Envoy,
    Traefik,
    AwsAlb,
    GcpLb,

    // Frontend / Backend
    NextJs,
    Nuxt,
    SvelteKit,
    Remix,
    Astro,
    Angular,
    React,
    NodeJs,
    Php,
    Python,
    Ruby,
    Go,
    Dotnet,
    Java,

    // Cache
    Varnish,
    Redis,

    // Analytics / Tagging
    GoogleAnalytics,
    AdobeAnalytics,
    Segment,
    Mixpanel,
    Hotjar,
    Plausible,
    Gtm,
    AdobeLaunch,
    Tealium,
    Optimizely,
    Vwo,
    GoogleOptimize,

    // Auth / Payment / Chat
    Auth0,
    Okta,
    Cognito,
    Clerk,
    NextAuth,
    Stripe,
    Adyen,
    Paypal,
    MercadoPago,
    Cielo,
    Intercom,
    Zendesk,
    Drift,
    JivoChat,

    // Hosting
    Aws,
    Gcp,
    Azure,
    DigitalOcean,
    Hetzner,
    Ovh,

    /// Vendor recognised by signal but name unknown / generic.
    Unknown,
    /// Used by sources that emit non-vendor evidence (e.g. block
    /// pattern without vendor identification).
    Generic,
}

impl Vendor {
    /// Display name suitable for reports.
    pub fn as_str(&self) -> &'static str {
        match self {
            Vendor::Cloudflare => "Cloudflare",
            Vendor::Akamai => "Akamai",
            Vendor::Fastly => "Fastly",
            Vendor::CloudFront => "CloudFront",
            Vendor::Bunny => "Bunny",
            Vendor::Vercel => "Vercel",
            Vendor::Netlify => "Netlify",
            Vendor::AwsWaf => "AWS WAF",
            Vendor::ImpervaWaf => "Imperva WAF",
            Vendor::SucuriWaf => "Sucuri",
            Vendor::F5BigIp => "F5 BIG-IP",
            Vendor::Wallarm => "Wallarm",
            Vendor::ModSecurity => "ModSecurity",
            Vendor::DataDome => "DataDome",
            Vendor::PerimeterX => "PerimeterX",
            Vendor::Imperva => "Imperva",
            Vendor::DistilNetworks => "DistilNetworks",
            Vendor::Kasada => "Kasada",
            Vendor::ShapeSecurity => "Shape Security",
            Vendor::AkamaiBotManager => "Akamai Bot Manager",
            Vendor::CloudflareBotManagement => "Cloudflare Bot Management",
            Vendor::CloudflareTurnstile => "Cloudflare Turnstile",
            Vendor::HCaptcha => "hCaptcha",
            Vendor::Recaptcha => "reCAPTCHA",
            Vendor::Wordpress => "WordPress",
            Vendor::Drupal => "Drupal",
            Vendor::Joomla => "Joomla",
            Vendor::Ghost => "Ghost",
            Vendor::Wagtail => "Wagtail",
            Vendor::Magento => "Magento",
            Vendor::Shopify => "Shopify",
            Vendor::Vtex => "VTEX",
            Vendor::BigCommerce => "BigCommerce",
            Vendor::SalesforceCommerce => "Salesforce Commerce",
            Vendor::WooCommerce => "WooCommerce",
            Vendor::Nginx => "Nginx",
            Vendor::Apache => "Apache",
            Vendor::Iis => "IIS",
            Vendor::Caddy => "Caddy",
            Vendor::LiteSpeed => "LiteSpeed",
            Vendor::OpenResty => "OpenResty",
            Vendor::Haproxy => "HAProxy",
            Vendor::Envoy => "Envoy",
            Vendor::Traefik => "Traefik",
            Vendor::AwsAlb => "AWS ALB",
            Vendor::GcpLb => "GCP LB",
            Vendor::NextJs => "Next.js",
            Vendor::Nuxt => "Nuxt",
            Vendor::SvelteKit => "SvelteKit",
            Vendor::Remix => "Remix",
            Vendor::Astro => "Astro",
            Vendor::Angular => "Angular",
            Vendor::React => "React",
            Vendor::NodeJs => "Node.js",
            Vendor::Php => "PHP",
            Vendor::Python => "Python",
            Vendor::Ruby => "Ruby",
            Vendor::Go => "Go",
            Vendor::Dotnet => ".NET",
            Vendor::Java => "Java",
            Vendor::Varnish => "Varnish",
            Vendor::Redis => "Redis",
            Vendor::GoogleAnalytics => "Google Analytics",
            Vendor::AdobeAnalytics => "Adobe Analytics",
            Vendor::Segment => "Segment",
            Vendor::Mixpanel => "Mixpanel",
            Vendor::Hotjar => "Hotjar",
            Vendor::Plausible => "Plausible",
            Vendor::Gtm => "Google Tag Manager",
            Vendor::AdobeLaunch => "Adobe Launch",
            Vendor::Tealium => "Tealium",
            Vendor::Optimizely => "Optimizely",
            Vendor::Vwo => "VWO",
            Vendor::GoogleOptimize => "Google Optimize",
            Vendor::Auth0 => "Auth0",
            Vendor::Okta => "Okta",
            Vendor::Cognito => "AWS Cognito",
            Vendor::Clerk => "Clerk",
            Vendor::NextAuth => "NextAuth",
            Vendor::Stripe => "Stripe",
            Vendor::Adyen => "Adyen",
            Vendor::Paypal => "PayPal",
            Vendor::MercadoPago => "MercadoPago",
            Vendor::Cielo => "Cielo",
            Vendor::Intercom => "Intercom",
            Vendor::Zendesk => "Zendesk",
            Vendor::Drift => "Drift",
            Vendor::JivoChat => "JivoChat",
            Vendor::Aws => "AWS",
            Vendor::Gcp => "GCP",
            Vendor::Azure => "Azure",
            Vendor::DigitalOcean => "DigitalOcean",
            Vendor::Hetzner => "Hetzner",
            Vendor::Ovh => "OVH",
            Vendor::Unknown => "Unknown",
            Vendor::Generic => "Generic",
        }
    }
}

/// What kind of thing the Vendor is. Determines which slot of
/// `FingerprintReport` the Detection goes into. Non-exhaustive so
/// adding a category is not a breaking change.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Cdn,
    Waf,
    Antibot,
    Cms,
    Ecommerce,
    Frontend,
    Backend,
    WebServer,
    ReverseProxyLb,
    Cache,
    Analytics,
    TagManager,
    AbTesting,
    Auth,
    Payment,
    Chat,
    DnsHosting,
    CookiePattern,
    Other,
}

/// When a Source runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Hot,
    Warm,
    Cold,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confidence_single_weight10_is_high() {
        let e = Evidence::new(EvidenceSource::Header, "cf-ray", 10);
        assert_eq!(Confidence::from_evidence(&[e]), Confidence::High);
    }

    #[test]
    fn confidence_sum_10_is_high() {
        let e1 = Evidence::new(EvidenceSource::Header, "h1", 6);
        let e2 = Evidence::new(EvidenceSource::CookieName, "c1", 4);
        assert_eq!(Confidence::from_evidence(&[e1, e2]), Confidence::High);
    }

    #[test]
    fn confidence_sum_6_is_medium() {
        let e1 = Evidence::new(EvidenceSource::Header, "h1", 3);
        let e2 = Evidence::new(EvidenceSource::CookieName, "c1", 3);
        assert_eq!(Confidence::from_evidence(&[e1, e2]), Confidence::Medium);
    }

    #[test]
    fn confidence_two_weight4_is_medium() {
        let e1 = Evidence::new(EvidenceSource::Header, "h1", 4);
        let e2 = Evidence::new(EvidenceSource::CookieName, "c1", 4);
        // sum=8 < 10 (not High), sum>=6 AND 2+ evidences weight>=4 → Medium
        assert_eq!(Confidence::from_evidence(&[e1, e2]), Confidence::Medium);
    }

    #[test]
    fn confidence_single_low_is_low() {
        let e = Evidence::new(EvidenceSource::TimingPattern, "ttfb", 2);
        assert_eq!(Confidence::from_evidence(&[e]), Confidence::Low);
    }

    #[test]
    fn confidence_empty_is_low() {
        assert_eq!(Confidence::from_evidence(&[]), Confidence::Low);
    }

    #[test]
    fn detection_single_constructor() {
        let d = Detection::from_single(
            Category::Cdn,
            Vendor::Cloudflare,
            Evidence::new(EvidenceSource::Header, "cf-ray=abc", 10),
        );
        assert_eq!(d.vendor, Vendor::Cloudflare);
        assert_eq!(d.category, Category::Cdn);
        assert_eq!(d.confidence, Confidence::High);
        assert_eq!(d.evidence.len(), 1);
    }

    #[test]
    fn evidence_weight_clamped() {
        let e = Evidence::new(EvidenceSource::Header, "x", 99);
        assert_eq!(e.weight, 10);
        let e0 = Evidence::new(EvidenceSource::Header, "x", 0);
        assert_eq!(e0.weight, 1);
    }
}
