//! URL asset classification.
//!
//! Labels every URL we see with one of a handful of kinds so the crawler can
//! reason about surface (pages), supporting assets, APIs, and binary payloads
//! separately. Classification uses extension first; MIME (when available from
//! a Content-Type header) refines or overrides the guess.

use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AssetKind {
    Page,
    Document,
    Image,
    Video,
    Audio,
    Font,
    Css,
    Js,
    Json,
    Xml,
    Sitemap,
    Robots,
    Feed,
    Archive,
    Binary,
    Manifest,
    Api,
    Other,
}

impl AssetKind {
    pub fn as_str(self) -> &'static str {
        use AssetKind::*;
        match self {
            Page => "page",
            Document => "document",
            Image => "image",
            Video => "video",
            Audio => "audio",
            Font => "font",
            Css => "css",
            Js => "js",
            Json => "json",
            Xml => "xml",
            Sitemap => "sitemap",
            Robots => "robots",
            Feed => "feed",
            Archive => "archive",
            Manifest => "manifest",
            Binary => "binary",
            Api => "api",
            Other => "other",
        }
    }

    pub fn is_page_like(self) -> bool {
        matches!(self, AssetKind::Page | AssetKind::Document)
    }

    /// Map an asset kind to the Chrome `Sec-Fetch-Dest` value. This drives
    /// HTTP header shaping so requests look like the browser actually issuing
    /// them instead of defaulting every fetch to a full-page navigation.
    pub fn sec_fetch_dest(self) -> SecFetchDest {
        use AssetKind::*;
        match self {
            Page => SecFetchDest::Document,
            Document => SecFetchDest::Document,
            Image => SecFetchDest::Image,
            Video => SecFetchDest::Video,
            Audio => SecFetchDest::Audio,
            Font => SecFetchDest::Font,
            Css => SecFetchDest::Style,
            Js => SecFetchDest::Script,
            Json | Xml | Sitemap | Robots | Feed | Manifest => SecFetchDest::Empty,
            Archive | Binary | Other => SecFetchDest::Empty,
            Api => SecFetchDest::Empty,
        }
    }
}

/// Minimal subset of the values Chrome emits in `Sec-Fetch-Dest`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecFetchDest {
    Document,
    Empty,
    Image,
    Script,
    Style,
    Font,
    Audio,
    Video,
}

impl SecFetchDest {
    pub fn as_str(self) -> &'static str {
        match self {
            SecFetchDest::Document => "document",
            SecFetchDest::Empty => "empty",
            SecFetchDest::Image => "image",
            SecFetchDest::Script => "script",
            SecFetchDest::Style => "style",
            SecFetchDest::Font => "font",
            SecFetchDest::Audio => "audio",
            SecFetchDest::Video => "video",
        }
    }

    /// Matching `Sec-Fetch-Mode` Chrome picks for the same dest.
    pub fn mode(self) -> &'static str {
        match self {
            SecFetchDest::Document => "navigate",
            SecFetchDest::Empty => "cors",
            _ => "no-cors",
        }
    }

    /// Chrome `Accept` header for the dest — these match what a real Chrome
    /// tab sends for each resource type.
    pub fn accept_header(self) -> &'static str {
        match self {
            SecFetchDest::Document =>
                "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7",
            SecFetchDest::Image =>
                "image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8",
            SecFetchDest::Style => "text/css,*/*;q=0.1",
            SecFetchDest::Script => "*/*",
            SecFetchDest::Font => "*/*",
            SecFetchDest::Audio | SecFetchDest::Video => "*/*",
            SecFetchDest::Empty => "*/*",
        }
    }

    pub fn is_document(self) -> bool {
        matches!(self, SecFetchDest::Document)
    }
}

pub fn classify_url(url: &Url) -> AssetKind {
    let path = url.path();
    if path == "/robots.txt" || path.ends_with("/robots.txt") {
        return AssetKind::Robots;
    }
    if path.ends_with("/sitemap.xml")
        || path.ends_with("/sitemap_index.xml")
        || path.contains("/sitemap")
    {
        return AssetKind::Sitemap;
    }
    // Last path segment (no query/fragment — url::Url::path() already excludes
    // those, but defensively strip anything past '?' or '#').
    let mut last = path.rsplit('/').next().unwrap_or("").to_ascii_lowercase();
    if let Some(i) = last.find('?') {
        last.truncate(i);
    }
    if let Some(i) = last.find('#') {
        last.truncate(i);
    }
    let ext = if last.contains('.') {
        last.rsplit('.').next().unwrap_or("")
    } else {
        ""
    };
    match ext {
        "html" | "htm" | "xhtml" => AssetKind::Page,
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "odt" | "rtf" | "txt" => {
            AssetKind::Document
        }
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "bmp" | "ico" | "svg" | "tiff"
        | "heic" | "heif" => AssetKind::Image,
        "mp4" | "webm" | "mkv" | "mov" | "avi" | "m4v" | "ts" | "m3u8" | "mpd" => AssetKind::Video,
        "mp3" | "wav" | "ogg" | "flac" | "opus" | "aac" | "m4a" => AssetKind::Audio,
        "woff" | "woff2" | "ttf" | "otf" | "eot" => AssetKind::Font,
        "css" => AssetKind::Css,
        "js" | "mjs" | "cjs" => AssetKind::Js,
        "json" => AssetKind::Json,
        "xml" => AssetKind::Xml,
        "rss" | "atom" => AssetKind::Feed,
        "zip" | "gz" | "tgz" | "bz2" | "xz" | "7z" | "rar" | "tar" => AssetKind::Archive,
        "webmanifest" | "manifest" => AssetKind::Manifest,
        "wasm" | "bin" | "exe" | "dmg" | "iso" | "deb" | "rpm" | "apk" => AssetKind::Binary,
        // No extension or unknown: look for common API path hints.
        _ => {
            if path.starts_with("/api/")
                || path.contains("/graphql")
                || path.contains("/rest/")
                || path.contains("/v1/")
                || path.contains("/v2/")
            {
                AssetKind::Api
            } else if ext.is_empty() {
                // Trailing slash or bare path: treat as page candidate.
                AssetKind::Page
            } else {
                AssetKind::Other
            }
        }
    }
}

pub fn classify_with_mime(url: &Url, content_type: Option<&str>) -> AssetKind {
    let mut k = classify_url(url);
    if let Some(ct) = content_type {
        let ct = ct.to_ascii_lowercase();
        let mime = ct.split(';').next().unwrap_or("").trim();
        let mime_kind = match mime {
            "text/html" | "application/xhtml+xml" => Some(AssetKind::Page),
            "application/pdf" => Some(AssetKind::Document),
            "text/css" => Some(AssetKind::Css),
            "application/javascript" | "text/javascript" | "application/ecmascript" => {
                Some(AssetKind::Js)
            }
            "application/json" | "application/ld+json" => Some(AssetKind::Json),
            "application/xml" | "text/xml" => Some(AssetKind::Xml),
            "application/rss+xml" | "application/atom+xml" => Some(AssetKind::Feed),
            "application/manifest+json" | "application/webmanifest" => Some(AssetKind::Manifest),
            "application/wasm" => Some(AssetKind::Binary),
            _ => None,
        };
        if let Some(mk) = mime_kind {
            // MIME wins when the extension was ambiguous/Other; otherwise we
            // trust the extension to disambiguate dynamic endpoints that serve
            // multiple kinds (e.g., /api/items.json).
            if matches!(k, AssetKind::Other | AssetKind::Page | AssetKind::Api) {
                k = mk;
            }
        }
        if mime.starts_with("image/") {
            k = AssetKind::Image;
        } else if mime.starts_with("video/") {
            k = AssetKind::Video;
        } else if mime.starts_with("audio/") {
            k = AssetKind::Audio;
        } else if mime.starts_with("font/") {
            k = AssetKind::Font;
        }
    }
    k
}
