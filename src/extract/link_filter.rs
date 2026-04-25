//! URL allow/deny list with domain, file-extension, subdomain,
//! social-media, robots.txt and regex include/exclude gates.
//!
//! Ported from Firecrawl `apps/api/native/src/crawler.rs` (MIT). The rules
//! and their ordering are kept identical so we stay bug-compatible with the
//! canonical behaviour crawler operators expect; changes are:
//!   * strip NAPI bindings;
//!   * use our `Url` and `Regex` types;
//!   * return `DenyReason` enum instead of the string tag table so callers
//!     can emit structured events.

use regex::Regex;
use std::collections::HashSet;
use std::sync::LazyLock;
use texting_robots::Robot;
use url::Url;

/// Extensions we treat as non-HTML assets; never enqueue a link ending in
/// one of these. Matches Firecrawl's FILE_EXTENSIONS list.
static FILE_EXTENSIONS: &[&str] = &[
    ".png", ".jpg", ".jpeg", ".gif", ".css", ".js", ".ico", ".svg", ".tiff", ".zip", ".exe",
    ".dmg", ".mp4", ".mp3", ".wav", ".pptx", ".xlsx", ".avi", ".flv", ".woff", ".ttf", ".woff2",
    ".webp", ".inc",
];

static FILE_EXT_SET: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| FILE_EXTENSIONS.iter().copied().collect());

/// URL schemes we never follow.
const NON_WEB_PROTOCOLS: &[&str] = &[
    "mailto:", "tel:", "telnet:", "ftp:", "ftps:", "ssh:", "file:",
];

/// Host substrings we treat as social/share destinations — enqueuing them
/// inflates the frontier with pages a crawler rarely actually wants.
const SOCIAL_MEDIA_OR_EMAIL: &[&str] = &[
    "facebook.com",
    "twitter.com",
    "linkedin.com",
    "instagram.com",
    "pinterest.com",
    "github.com",
    "calendly.com",
    "discord.gg",
    "discord.com",
];

/// Why a candidate link was rejected. Structured so callers can emit
/// `decision.made why=...` events without parsing strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenyReason {
    UrlParse,
    DepthLimit,
    FileType,
    SectionLink,
    BackwardCrawling,
    ExcludePattern,
    IncludePattern,
    RobotsTxt,
    NonWebProtocol,
    SocialMedia,
    ExternalLink,
}

impl DenyReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UrlParse => "url-parse",
            Self::DepthLimit => "depth-limit",
            Self::FileType => "file-type",
            Self::SectionLink => "section-link",
            Self::BackwardCrawling => "backward-crawling",
            Self::ExcludePattern => "exclude-pattern",
            Self::IncludePattern => "include-pattern",
            Self::RobotsTxt => "robots-txt",
            Self::NonWebProtocol => "non-web-protocol",
            Self::SocialMedia => "social-media",
            Self::ExternalLink => "external-link",
        }
    }
}

/// Parameters for [`filter_links`]. Mirrors Firecrawl's `FilterLinksCall`
/// minus the FFI object shape.
pub struct FilterLinksInput<'a> {
    pub links: Vec<String>,
    pub limit: Option<usize>,
    pub max_depth: u32,
    pub base_url: &'a Url,
    pub initial_url: &'a Url,
    pub regex_on_full_url: bool,
    pub excludes: &'a [Regex],
    pub includes: &'a [Regex],
    pub allow_backward_crawling: bool,
    pub ignore_robots_txt: bool,
    pub robots_txt: &'a str,
    pub robots_user_agent: Option<&'a str>,
    pub allow_external_content_links: bool,
    pub allow_subdomains: bool,
}

#[derive(Debug, Clone)]
pub struct FilterLinksResult {
    pub links: Vec<String>,
    pub denials: Vec<(String, DenyReason)>,
}

pub fn is_file(path: &str) -> bool {
    if let Some(dot_pos) = path.rfind('.') {
        let extension = &path[dot_pos..].to_ascii_lowercase();
        FILE_EXT_SET.contains(extension.as_str())
    } else {
        false
    }
}

pub fn get_url_depth(path: &str) -> u32 {
    path.split('/')
        .filter(|seg| !seg.is_empty() && *seg != "index.php" && *seg != "index.html")
        .count() as u32
}

pub fn is_internal_link(url: &Url, base_url: &Url) -> bool {
    let base_domain = url_host_bare(base_url);
    let link_domain = url_host_bare(url);
    link_domain == base_domain
}

fn url_host_bare(u: &Url) -> String {
    u.host_str()
        .unwrap_or("")
        .trim_start_matches("www.")
        .trim()
        .to_ascii_lowercase()
}

/// A fragment `#foo` counts as a "section" and should be skipped; but a
/// hash-router path like `#/dashboard/page` is a real page.
pub fn no_sections(url_str: &str) -> bool {
    if !url_str.contains('#') {
        return true;
    }
    if let Some(hash_part) = url_str.split('#').nth(1) {
        hash_part.len() > 1 && hash_part.contains('/')
    } else {
        false
    }
}

pub fn is_non_web_protocol(url_str: &str) -> bool {
    NON_WEB_PROTOCOLS.iter().any(|p| url_str.starts_with(p))
}

pub fn is_social_media_or_email(url_str: &str) -> bool {
    SOCIAL_MEDIA_OR_EMAIL
        .iter()
        .any(|domain| url_str.contains(domain))
}

/// Are `url` and `base_url` on the same registrable domain (via PSL)?
pub fn is_subdomain(url: &Url, base_url: &Url) -> bool {
    match (url.host_str(), base_url.host_str()) {
        (Some(link_host), Some(base_host)) => {
            match (psl::domain_str(link_host), psl::domain_str(base_host)) {
                (Some(a), Some(b)) => a == b,
                _ => false,
            }
        }
        _ => false,
    }
}

pub fn is_external_main_page(url_str: &str) -> bool {
    if let Ok(url) = Url::parse(url_str) {
        let segs: Vec<&str> = url
            .path_segments()
            .map(|s| s.filter(|x| !x.is_empty()).collect())
            .unwrap_or_default();
        segs.is_empty()
    } else {
        false
    }
}

fn build_robot(
    ignore_robots_txt: bool,
    robots_txt: &str,
    robots_user_agent: Option<&str>,
) -> Option<Robot> {
    if ignore_robots_txt || robots_txt.is_empty() {
        return None;
    }
    if let Some(ua) = robots_user_agent {
        return Robot::new(ua, robots_txt.as_bytes()).ok();
    }
    Robot::new("crawlex", robots_txt.as_bytes()).ok()
}

/// Filter a batch of discovered links against every gate. Ordering of the
/// checks is intentional — cheap textual rejects come first.
pub fn filter_links(data: FilterLinksInput<'_>) -> FilterLinksResult {
    let limit = data.limit.unwrap_or(usize::MAX);
    if limit == 0 {
        return FilterLinksResult {
            links: Vec::new(),
            denials: Vec::new(),
        };
    }

    let initial_path = data.initial_url.path().to_string();
    let robot = build_robot(
        data.ignore_robots_txt,
        data.robots_txt,
        data.robots_user_agent,
    );

    let mut out = Vec::new();
    let mut denials: Vec<(String, DenyReason)> = Vec::new();

    let push_deny = |denials: &mut Vec<(String, DenyReason)>, link: String, r: DenyReason| {
        denials.push((link, r));
    };

    for link in data.links {
        if out.len() >= limit {
            break;
        }

        let url = match data.base_url.join(&link) {
            Ok(u) => u,
            Err(_) => {
                push_deny(&mut denials, link, DenyReason::UrlParse);
                continue;
            }
        };

        let path = url.path().to_string();
        let url_str = url.as_str().to_string();

        if is_non_web_protocol(&url_str) {
            push_deny(&mut denials, link, DenyReason::NonWebProtocol);
            continue;
        }

        if get_url_depth(&path) > data.max_depth {
            push_deny(&mut denials, link, DenyReason::DepthLimit);
            continue;
        }

        if is_file(&path) {
            push_deny(&mut denials, link, DenyReason::FileType);
            continue;
        }

        if is_internal_link(&url, data.base_url) {
            if !no_sections(&url_str) {
                push_deny(&mut denials, link, DenyReason::SectionLink);
                continue;
            }

            if !data.allow_backward_crawling && !path.starts_with(&initial_path) {
                push_deny(&mut denials, link, DenyReason::BackwardCrawling);
                continue;
            }

            let match_target: &str = if data.regex_on_full_url {
                &url_str
            } else {
                &path
            };

            if !data.excludes.is_empty() && data.excludes.iter().any(|r| r.is_match(match_target)) {
                push_deny(&mut denials, link, DenyReason::ExcludePattern);
                continue;
            }

            if !data.includes.is_empty() && !data.includes.iter().any(|r| r.is_match(match_target))
            {
                push_deny(&mut denials, link, DenyReason::IncludePattern);
                continue;
            }

            if let Some(ref robot) = robot {
                if !robot.allowed(&url_str) {
                    push_deny(&mut denials, link, DenyReason::RobotsTxt);
                    continue;
                }
            }

            out.push(link);
        } else {
            // External link path.
            if is_social_media_or_email(&url_str) {
                push_deny(&mut denials, link, DenyReason::SocialMedia);
                continue;
            }

            if !data.excludes.is_empty() && data.excludes.iter().any(|r| r.is_match(&url_str)) {
                push_deny(&mut denials, link, DenyReason::ExcludePattern);
                continue;
            }

            if is_internal_link(data.initial_url, data.base_url)
                && data.allow_external_content_links
                && !is_external_main_page(&url_str)
            {
                out.push(link);
                continue;
            }

            if data.allow_subdomains
                && !is_social_media_or_email(&url_str)
                && is_subdomain(&url, data.base_url)
            {
                let match_target: &str = if data.regex_on_full_url {
                    &url_str
                } else {
                    &path
                };
                if !data.includes.is_empty()
                    && !data.includes.iter().any(|r| r.is_match(match_target))
                {
                    push_deny(&mut denials, link, DenyReason::IncludePattern);
                    continue;
                }
                out.push(link);
                continue;
            }

            push_deny(&mut denials, link, DenyReason::ExternalLink);
        }
    }

    FilterLinksResult {
        links: out,
        denials,
    }
}
