"""Access profiles for measured, policy-driven crawling.

This module intentionally keeps the first implementation requests-based. It
models the pieces we already measured for Drogaria Sao Paulo:

- sticky identity: one shared requests.Session, connection reuse keeps exit IP;
- rotating identity: a fresh requests.Session per request;
- per-profile rate limits and circuit breakers;
- response classification for CloudFront/WAF blocks;
- optional proxy URLs sourced from environment variables.

It does not try to emulate browser TLS/JS fingerprints. A future BrowserClient
can implement the same high-level AccessProfile contract for Playwright/Camoufox.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Literal
from urllib.parse import quote
import json
import os
import random
import re
import time

import requests
import urllib3

urllib3.disable_warnings(urllib3.exceptions.InsecureRequestWarning)

SessionMode = Literal["shared", "isolated"]
EndpointType = Literal["vtex_api", "html", "ip_check"]


@dataclass(frozen=True)
class ProxyProfile:
    name: str
    provider: str = "packetstream"
    url_env: str | None = None
    url: str | None = None
    country: str | None = None
    region: str | None = None
    city: str | None = None
    mode: Literal["sticky", "rotating", "static", "unknown"] = "unknown"
    protocol: str = "socks5h"


@dataclass(frozen=True)
class HardwareProfile:
    name: str
    device_type: Literal["desktop", "mobile", "tablet"] = "desktop"
    os: str = "linux"
    browser: str = "chrome"
    user_agent: str = (
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 "
        "(KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36"
    )
    accept_language: str = "pt-BR,pt;q=0.9,en;q=0.5"
    timezone: str | None = "America/Sao_Paulo"
    viewport: tuple[int, int] | None = None
    sec_ch_ua: str | None = None
    sec_ch_ua_mobile: str | None = None
    sec_ch_ua_platform: str | None = None
    accept: str | None = None
    extra_headers: dict[str, str] = field(default_factory=dict)


@dataclass(frozen=True)
class SessionProfile:
    name: str
    mode: SessionMode = "shared"
    cookie_policy: Literal["memory", "persist", "discard"] = "memory"
    connection_reuse: bool = True
    max_requests_per_session: int | None = None
    max_session_age_seconds: float | None = None


@dataclass(frozen=True)
class RateLimitProfile:
    min_delay_seconds: float = 3.0
    jitter_seconds: float = 0.4
    max_retries: int = 1
    backoff_base_seconds: float = 2.0
    circuit_breaker_threshold: int = 3
    circuit_breaker_cooldown_seconds: float = 900.0


@dataclass(frozen=True)
class AccessProfile:
    name: str
    target: str
    proxy: ProxyProfile
    hardware: HardwareProfile
    session: SessionProfile
    rate_limit: RateLimitProfile
    allowed_endpoint_types: tuple[EndpointType, ...] = ("vtex_api",)
    require_product_links: bool = False
    identity_pool: str | None = None
    identity_role: Literal["primary", "fallback", "benchmark", "disabled"] = "primary"
    identity_weight: int = 100
    sticky_identity_id: str | None = None
    notes: str | None = None


@dataclass
class CrawlResponse:
    profile: str
    endpoint_type: EndpointType
    url: str
    status_code: int | None
    ok: bool
    blocked: bool
    block_reason: str | None
    elapsed_ms: int
    bytes: int
    item_count: int | None = None
    product_link_count: int | None = None
    html_title: str | None = None
    exit_ip: str | None = None
    country: str | None = None
    error: str | None = None
    attempt: int = 1

    def to_json(self) -> dict[str, Any]:
        return self.__dict__.copy()


@dataclass
class _ManagedSession:
    session: requests.Session
    created_at: float
    request_count: int = 0


def load_dotenv(path: Path) -> None:
    if not path.exists():
        return
    for raw in path.read_text().splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        os.environ.setdefault(key.strip(), value.strip().strip('"').strip("'"))


def default_packetstream_proxy_url() -> str | None:
    enabled = os.environ.get("CRAWLEX_PROXY_ENABLED", "true").lower() in {"1", "true", "yes", "on"}
    if not enabled:
        return None
    host = os.environ.get("CRAWLEX_PROXY_HOST")
    port = os.environ.get("CRAWLEX_PROXY_PORT")
    user = os.environ.get("CRAWLEX_PROXY_USER")
    password = os.environ.get("CRAWLEX_PROXY_PASSWORD")
    if not all([host, port, user, password]):
        return None
    scheme = os.environ.get("CRAWLEX_PROXY_SCHEME", "socks5h")
    return f"{scheme}://{quote(user or '', safe='')}:{quote(password or '', safe='')}@{host}:{port}"


def resolve_proxy_url(profile: ProxyProfile) -> str | None:
    if profile.url_env:
        value = os.environ.get(profile.url_env)
        if value:
            return value
    if profile.url:
        return profile.url
    return default_packetstream_proxy_url()


def redact_secret(text: str) -> str:
    return re.sub(r"(socks5h?|https?|http)://[^@\s]+@", r"\1://[REDACTED]@", text)


def classify_response(status: int | None, text: str, json_list: bool) -> tuple[bool, str | None]:
    lower = text.lower()
    if status in (401, 407):
        return True, "proxy_or_auth_required"
    if status == 429:
        return True, "rate_limited_429"
    if status == 403 and "request could not be satisfied" in lower:
        return True, "cloudfront_request_could_not_be_satisfied"
    if status == 403:
        return True, "forbidden_403"
    if "captcha" in lower:
        return True, "captcha_or_challenge"
    if status and status >= 500:
        return True, "server_error"
    return False, None


def make_headers(profile: AccessProfile, endpoint_type: EndpointType, url: str | None = None) -> dict[str, str]:
    hardware = profile.hardware
    headers = {
        "User-Agent": hardware.user_agent,
        "Accept-Language": hardware.accept_language,
        "Cache-Control": "no-cache",
        "Pragma": "no-cache",
    }
    if hardware.sec_ch_ua:
        headers["sec-ch-ua"] = hardware.sec_ch_ua
    if hardware.sec_ch_ua_mobile:
        headers["sec-ch-ua-mobile"] = hardware.sec_ch_ua_mobile
    if hardware.sec_ch_ua_platform:
        headers["sec-ch-ua-platform"] = hardware.sec_ch_ua_platform
    if endpoint_type == "vtex_api":
        headers.update(
            {
                "Accept": hardware.accept or "application/json,text/plain,*/*",
                "Referer": "https://www.drogariasaopaulo.com.br/",
                "Sec-Fetch-Site": "same-origin",
                "Sec-Fetch-Mode": "cors",
                "Sec-Fetch-Dest": "empty",
            }
        )
    else:
        headers.update(
            {
                "Accept": hardware.accept or "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
                "Upgrade-Insecure-Requests": "1",
                "Sec-Fetch-Site": "none" if not url else "same-origin",
                "Sec-Fetch-Mode": "navigate",
                "Sec-Fetch-Dest": "document",
            }
        )
    headers.update(hardware.extra_headers)
    return headers


def validate_profile(profile: AccessProfile) -> list[str]:
    """Return human-readable profile consistency warnings.

    This is not a stealth guarantee. It catches obvious contradictions that make
    crawler traffic noisy, such as a Windows UA with Linux Client Hints or a
    mobile UA paired with `sec-ch-ua-mobile: ?0`.
    """
    warnings: list[str] = []
    hw = profile.hardware
    ua = hw.user_agent.lower()
    platform_hint = (hw.sec_ch_ua_platform or "").lower()
    mobile_hint = (hw.sec_ch_ua_mobile or "").strip()
    if hw.browser == "chrome" and hw.sec_ch_ua is None:
        warnings.append("chrome profile missing sec-ch-ua")
    if hw.os == "windows" and ("windows" not in ua or (platform_hint and "windows" not in platform_hint)):
        warnings.append("windows profile has non-windows UA/client-hint")
    if hw.os == "linux" and "linux" not in ua and "x11" not in ua:
        warnings.append("linux profile has non-linux UA")
    if hw.os == "android" and ("android" not in ua or (platform_hint and "android" not in platform_hint)):
        warnings.append("android profile has non-android UA/client-hint")
    if hw.device_type == "mobile" and mobile_hint == "?0":
        warnings.append("mobile profile has sec-ch-ua-mobile ?0")
    if hw.device_type == "desktop" and mobile_hint == "?1":
        warnings.append("desktop profile has sec-ch-ua-mobile ?1")
    if hw.timezone == "America/Sao_Paulo" and "pt-BR" not in hw.accept_language:
        warnings.append("Brazil timezone profile should prefer pt-BR language")
    if profile.proxy.mode == "sticky" and profile.session.mode != "shared":
        warnings.append("sticky proxy intent should use shared session mode")
    if profile.proxy.mode == "rotating" and profile.session.mode != "isolated":
        warnings.append("rotating proxy intent should use isolated session mode")
    if profile.identity_role in {"primary", "fallback"} and profile.proxy.mode == "rotating":
        warnings.append("production identity should not be per-request rotating")
    if profile.identity_role in {"primary", "fallback"} and not profile.sticky_identity_id:
        warnings.append("production identity should declare sticky_identity_id")
    return warnings


def profiles_in_pool(
    profiles: dict[str, AccessProfile],
    pool: str,
    endpoint_type: EndpointType = "vtex_api",
    roles: tuple[str, ...] = ("primary", "fallback"),
) -> list[AccessProfile]:
    """Return stable identities from a named pool, highest efficiency first.

    This excludes benchmark/disabled profiles from production selection. It sorts
    by measured/preferred weight and then name for deterministic behavior:
    variation is across a small set of coherent identities, not random headers or
    a fresh proxy IP for every request.
    """
    selected = [
        profile
        for profile in profiles.values()
        if profile.identity_pool == pool
        and profile.identity_role in roles
        and endpoint_type in profile.allowed_endpoint_types
    ]
    selected.sort(key=lambda p: (-p.identity_weight, p.identity_role != "primary", p.name))
    return selected


def vtex_api_url(term: str) -> str:
    return f"https://www.drogariasaopaulo.com.br/api/catalog_system/pub/products/search/{quote(term)}"


def html_search_url(term: str) -> str:
    return f"https://www.drogariasaopaulo.com.br/search?w={quote(term)}"


class CircuitBreakerOpen(RuntimeError):
    pass


class RequestProfileClient:
    def __init__(self, profile: AccessProfile):
        self.profile = profile
        self._shared: _ManagedSession | None = None
        self._last_request_at = 0.0
        self._consecutive_blocks = 0
        self._circuit_open_until = 0.0

    def _new_session(self) -> _ManagedSession:
        session = requests.Session()
        proxy_url = resolve_proxy_url(self.profile.proxy)
        if proxy_url:
            session.proxies.update({"http": proxy_url, "https": proxy_url})
        session.headers.update(make_headers(self.profile, "vtex_api"))
        return _ManagedSession(session=session, created_at=time.monotonic())

    def _session_expired(self, managed: _ManagedSession) -> bool:
        max_requests = self.profile.session.max_requests_per_session
        max_age = self.profile.session.max_session_age_seconds
        if max_requests is not None and managed.request_count >= max_requests:
            return True
        if max_age is not None and (time.monotonic() - managed.created_at) >= max_age:
            return True
        return False

    def _get_session(self) -> _ManagedSession:
        if self.profile.session.mode == "isolated":
            return self._new_session()
        if self._shared is None or self._session_expired(self._shared):
            self._shared = self._new_session()
        return self._shared

    def _wait_rate_limit(self) -> None:
        now = time.monotonic()
        delay = self.profile.rate_limit.min_delay_seconds + random.uniform(0, self.profile.rate_limit.jitter_seconds)
        elapsed = now - self._last_request_at
        if self._last_request_at and elapsed < delay:
            time.sleep(delay - elapsed)

    def _check_circuit(self) -> None:
        if time.monotonic() < self._circuit_open_until:
            remaining = int(self._circuit_open_until - time.monotonic())
            raise CircuitBreakerOpen(f"profile {self.profile.name} circuit open for {remaining}s")

    def request(self, url: str, endpoint_type: EndpointType, timeout: float = 35.0) -> requests.Response:
        """Perform a policy-controlled HTTP GET and return the raw response.

        Production loaders use this when they need full JSON/HTML bodies while
        preserving the same profile session, proxy, rate-limit, and
        circuit-breaker behavior measured by the benchmark runner.
        """
        if endpoint_type not in self.profile.allowed_endpoint_types and endpoint_type != "ip_check":
            raise ValueError(f"profile {self.profile.name} does not allow endpoint {endpoint_type}")
        self._check_circuit()
        self._wait_rate_limit()
        managed = self._get_session()
        headers = make_headers(self.profile, endpoint_type, url)
        response = managed.session.get(url, headers=headers, timeout=timeout, verify=False, allow_redirects=True)
        managed.request_count += 1
        self._last_request_at = time.monotonic()
        blocked, _reason = classify_response(response.status_code, response.text[:1200], False)
        if blocked:
            self._consecutive_blocks += 1
            threshold = self.profile.rate_limit.circuit_breaker_threshold
            if threshold and self._consecutive_blocks >= threshold:
                self._circuit_open_until = time.monotonic() + self.profile.rate_limit.circuit_breaker_cooldown_seconds
        else:
            self._consecutive_blocks = 0
        return response

    def fetch_url(self, url: str, endpoint_type: EndpointType, timeout: float = 35.0, attempt: int = 1) -> CrawlResponse:
        if endpoint_type not in self.profile.allowed_endpoint_types and endpoint_type != "ip_check":
            raise ValueError(f"profile {self.profile.name} does not allow endpoint {endpoint_type}")
        self._check_circuit()
        self._wait_rate_limit()
        managed = self._get_session()
        headers = make_headers(self.profile, endpoint_type, url)
        started = time.perf_counter()
        status = None
        raw_len = 0
        text = ""
        json_list = False
        item_count = None
        product_link_count = None
        html_title = None
        exit_ip = None
        country = None
        error = None
        try:
            response = managed.session.get(url, headers=headers, timeout=timeout, verify=False, allow_redirects=True)
            managed.request_count += 1
            self._last_request_at = time.monotonic()
            status = response.status_code
            raw_len = len(response.content)
            text = response.text[:1200]
            content_type = response.headers.get("content-type", "")
            if endpoint_type == "ip_check":
                stripped = response.text.strip()
                exit_ip = stripped if stripped else None
            if "json" in content_type or endpoint_type == "vtex_api":
                try:
                    data = response.json()
                    json_list = isinstance(data, list)
                    if json_list:
                        item_count = len(data)
                except Exception:
                    json_list = False
            if endpoint_type == "html":
                title_match = re.search(r"<title[^>]*>(.*?)</title>", response.text, re.I | re.S)
                if title_match:
                    html_title = re.sub(r"\s+", " ", title_match.group(1)).strip()[:180]
                product_link_count = len(set(re.findall(r'href=["\']([^"\']*/p(?:[?#][^"\']*)?)["\']', response.text, re.I)))
        except Exception as exc:
            error = redact_secret(f"{type(exc).__name__}: {exc}")[:500]
            self._last_request_at = time.monotonic()
        elapsed_ms = int((time.perf_counter() - started) * 1000)
        blocked, reason = classify_response(status, text, json_list)
        ok = bool(
            (endpoint_type == "vtex_api" and status in (200, 206) and json_list)
            or (endpoint_type == "html" and status == 200 and not blocked and (not self.profile.require_product_links or (product_link_count or 0) > 0))
            or (endpoint_type == "ip_check" and status == 200 and not blocked and exit_ip)
        )
        if blocked:
            self._consecutive_blocks += 1
            threshold = self.profile.rate_limit.circuit_breaker_threshold
            if threshold and self._consecutive_blocks >= threshold:
                self._circuit_open_until = time.monotonic() + self.profile.rate_limit.circuit_breaker_cooldown_seconds
        elif ok:
            self._consecutive_blocks = 0
        return CrawlResponse(
            profile=self.profile.name,
            endpoint_type=endpoint_type,
            url=url,
            status_code=status,
            ok=ok,
            blocked=blocked,
            block_reason=reason,
            elapsed_ms=elapsed_ms,
            bytes=raw_len,
            item_count=item_count,
            product_link_count=product_link_count,
            html_title=html_title,
            exit_ip=exit_ip,
            country=country,
            error=error,
            attempt=attempt,
        )

    def fetch_term(self, term: str, endpoint_type: EndpointType, timeout: float = 35.0) -> CrawlResponse:
        url = vtex_api_url(term) if endpoint_type == "vtex_api" else html_search_url(term)
        attempts = max(1, self.profile.rate_limit.max_retries + 1)
        last: CrawlResponse | None = None
        for attempt in range(1, attempts + 1):
            last = self.fetch_url(url, endpoint_type, timeout=timeout, attempt=attempt)
            if last.ok or attempt == attempts:
                return last
            time.sleep(self.profile.rate_limit.backoff_base_seconds * attempt)
        assert last is not None
        return last


def _obj(data: dict[str, Any], cls: type[Any]) -> Any:
    return cls(**data)


def load_profiles(path: Path) -> dict[str, AccessProfile]:
    data = json.loads(path.read_text())
    profiles: dict[str, AccessProfile] = {}
    for name, raw in data.get("profiles", {}).items():
        proxy = _obj(raw.get("proxy", {}) | {"name": raw.get("proxy", {}).get("name", f"{name}-proxy")}, ProxyProfile)
        hardware = _obj(raw.get("hardware", {}) | {"name": raw.get("hardware", {}).get("name", f"{name}-hardware")}, HardwareProfile)
        session = _obj(raw.get("session", {}) | {"name": raw.get("session", {}).get("name", f"{name}-session")}, SessionProfile)
        rate_limit = _obj(raw.get("rate_limit", {}), RateLimitProfile)
        endpoint_types = tuple(raw.get("allowed_endpoint_types", ["vtex_api"]))
        profiles[name] = AccessProfile(
            name=name,
            target=raw.get("target", name),
            proxy=proxy,
            hardware=hardware,
            session=session,
            rate_limit=rate_limit,
            allowed_endpoint_types=endpoint_types,  # type: ignore[arg-type]
            require_product_links=bool(raw.get("require_product_links", False)),
            identity_pool=raw.get("identity_pool"),
            identity_role=raw.get("identity_role", "primary"),
            identity_weight=int(raw.get("identity_weight", 100)),
            sticky_identity_id=raw.get("sticky_identity_id"),
            notes=raw.get("notes"),
        )
    return profiles
