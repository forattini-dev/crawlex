#!/usr/bin/env python3
"""Benchmark conservative Drogaria Sao Paulo access patterns via PacketStream.

Goals:
- Compare shared vs isolated requests.Session usage.
- Compare conservative per-request delays.
- Exercise VTEX API and optional HTML endpoints.
- Support PacketStream-style PACKETSTREAM_PROXY_URLS comma-separated proxies,
  while also supporting the project's CRAWLEX_PROXY_* .env.

This script never prints proxy credentials or full proxy URLs.
"""

from __future__ import annotations

import argparse
import json
import os
import random
import re
import statistics
import sys
import time
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Iterable
from urllib.parse import quote

import requests
import urllib3

ROOT = Path(__file__).resolve().parents[1]
ENV_FILE = Path(os.environ.get("CRAWLEX_ENV_FILE", ROOT / ".env"))
OUT_DIR = ROOT / "data" / "benchmarks"
urllib3.disable_warnings(urllib3.exceptions.InsecureRequestWarning)

DSP_BASE = "https://www.drogariasaopaulo.com.br"
DEFAULT_TERMS = ["dipirona", "paracetamol", "losartana", "ibuprofeno", "tadalafila"]

USER_AGENTS = [
    # Keep a small, realistic set for comparison. Default run uses the first UA
    # unless --rotate-user-agent is explicitly set.
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 13_6) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36",
]

SECRET_PATTERNS = [
    re.compile(r"(socks5h?://)[^\s/@:]+:[^\s/@]+@", re.I),
    re.compile(r"(https?://)[^\s/@:]+:[^\s/@]+@", re.I),
    re.compile(r"(CRAWLEX_PROXY_PASSWORD=).*", re.I),
    re.compile(r"(CRAWLEX_PROXY_USER=).*", re.I),
]


def redact(value: object) -> str:
    text = str(value)
    for pattern in SECRET_PATTERNS:
        if "socks" in pattern.pattern or "https?" in pattern.pattern:
            text = pattern.sub(r"\1[REDACTED]@", text)
        else:
            text = pattern.sub(r"\1[REDACTED]", text)
    return text


def load_env(path: Path = ENV_FILE) -> None:
    if not path.exists():
        return
    for raw in path.read_text().splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        os.environ.setdefault(key.strip(), value.strip().strip('"').strip("'"))


def crawlex_proxy_url() -> str | None:
    enabled = os.environ.get("CRAWLEX_PROXY_ENABLED", "true").lower() not in {"0", "false", "no", "off"}
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


def proxy_urls() -> list[str]:
    load_env()
    raw = os.environ.get("PACKETSTREAM_PROXY_URLS", "").strip()
    urls = [u.strip() for u in raw.split(",") if u.strip()] if raw else []
    project_proxy = crawlex_proxy_url()
    if project_proxy and project_proxy not in urls:
        urls.append(project_proxy)
    return urls


def proxy_dict(proxy_url: str | None) -> dict[str, str] | None:
    if not proxy_url:
        return None
    # requests supports socks5h when PySocks is installed; keep scheme as-is.
    return {"http": proxy_url, "https": proxy_url}


def headers(user_agent: str, accept_json: bool, referer: str | None = None) -> dict[str, str]:
    h = {
        "User-Agent": user_agent,
        "Accept-Language": "pt-BR,pt;q=0.9,en;q=0.5",
        "Cache-Control": "no-cache",
        "Pragma": "no-cache",
    }
    if accept_json:
        h["Accept"] = "application/json,text/plain,*/*"
    else:
        h["Accept"] = "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8"
    if referer:
        h["Referer"] = referer
    return h


@dataclass
class Result:
    ts: float
    scenario: str
    session_mode: str
    endpoint_type: str
    term: str
    url: str
    proxy_label: str
    user_agent_label: str
    status_code: int | None
    ok: bool
    blocked: bool
    block_reason: str | None
    json_list: bool
    item_count: int | None
    html_title: str | None
    product_link_count: int | None
    bytes: int
    elapsed_ms: int
    error: str | None


def classify(status: int | None, text: str, json_list: bool) -> tuple[bool, str | None]:
    if status in (403, 407):
        if "request could not be satisfied" in text.lower():
            return True, "cloudfront_request_could_not_be_satisfied"
        return True, f"http_{status}"
    if status == 429:
        return True, "rate_limited_429"
    if status and 500 <= status < 600:
        return True, f"server_{status}"
    if text and any(marker in text.lower() for marker in ["captcha", "access denied", "blocked"]):
        return True, "soft_block_marker"
    return False, None


def make_session(proxy_url: str | None, user_agent: str) -> requests.Session:
    s = requests.Session()
    p = proxy_dict(proxy_url)
    if p:
        s.proxies.update(p)
    s.headers.update(headers(user_agent, accept_json=False))
    return s


def request_once(
    session: requests.Session,
    scenario: str,
    session_mode: str,
    endpoint_type: str,
    term: str,
    url: str,
    proxy_label: str,
    user_agent_label: str,
    timeout: float,
) -> Result:
    t0 = time.perf_counter()
    status = None
    text = ""
    raw_len = 0
    json_list = False
    item_count = None
    html_title = None
    product_link_count = None
    err = None
    try:
        ua = str(session.headers.get("User-Agent", USER_AGENTS[0]))
        if endpoint_type == "api":
            req_headers = headers(ua, True, f"{DSP_BASE}/search?w={quote(term)}")
        else:
            req_headers = headers(ua, False, DSP_BASE + "/")
        r = session.get(url, headers=req_headers, timeout=timeout, allow_redirects=True, verify=False)
        status = r.status_code
        raw_len = len(r.content)
        text = r.text[:400]
        if endpoint_type == "html":
            title_match = re.search(r"<title[^>]*>(.*?)</title>", r.text, re.I | re.S)
            if title_match:
                html_title = re.sub(r"\s+", " ", title_match.group(1)).strip()[:180]
            product_link_count = len(set(re.findall(r'href=["\']([^"\']*/p(?:[?#][^"\']*)?)["\']', r.text, re.I)))
        try:
            data = r.json()
            json_list = isinstance(data, list)
            item_count = len(data) if json_list else None
        except Exception:
            pass
    except Exception as exc:  # noqa: BLE001 - benchmark records transport errors.
        err = redact(f"{type(exc).__name__}: {exc}")[:500]
    elapsed = int((time.perf_counter() - t0) * 1000)
    blocked, reason = classify(status, text, json_list)
    ok = bool(
        (endpoint_type == "api" and status in (200, 206) and json_list)
        or (endpoint_type == "html" and status == 200 and not blocked and (product_link_count or 0) > 0)
    )
    return Result(
        ts=time.time(),
        scenario=scenario,
        session_mode=session_mode,
        endpoint_type=endpoint_type,
        term=term,
        url=url,
        proxy_label=proxy_label,
        user_agent_label=user_agent_label,
        status_code=status,
        ok=ok,
        blocked=blocked,
        block_reason=reason,
        json_list=json_list,
        item_count=item_count,
        html_title=html_title,
        product_link_count=product_link_count,
        bytes=raw_len,
        elapsed_ms=elapsed,
        error=err,
    )


def endpoint_url(endpoint_type: str, term: str) -> str:
    if endpoint_type == "api":
        return f"{DSP_BASE}/api/catalog_system/pub/products/search/{quote(term)}"
    if endpoint_type == "html":
        return f"{DSP_BASE}/search?w={quote(term)}"
    raise ValueError(endpoint_type)


def run_scenario(args: argparse.Namespace, scenario: str, session_mode: str, endpoint_type: str, terms: list[str], delay: float, proxy_url: str | None) -> list[Result]:
    out: list[Result] = []
    proxy_label = "direct" if not proxy_url else redact(proxy_url).split("@")[0] + "@[redacted-host]"
    shared_session: requests.Session | None = None
    if session_mode == "shared":
        ua = USER_AGENTS[0]
        shared_session = make_session(proxy_url, ua)
    for i in range(args.requests):
        term = terms[i % len(terms)]
        ua_idx = (i % len(USER_AGENTS)) if args.rotate_user_agent else 0
        ua = USER_AGENTS[ua_idx]
        user_agent_label = f"ua{ua_idx}"
        if session_mode == "shared":
            assert shared_session is not None
            session = shared_session
        else:
            session = make_session(proxy_url, ua)
        url = endpoint_url(endpoint_type, term)
        res = request_once(session, scenario, session_mode, endpoint_type, term, url, proxy_label, user_agent_label, args.timeout)
        out.append(res)
        print(
            f"{scenario} #{i+1}/{args.requests} {endpoint_type} {session_mode} term={term} "
            f"status={res.status_code} ok={res.ok} blocked={res.blocked} reason={res.block_reason} "
            f"items={res.item_count} product_links={res.product_link_count} title={res.html_title or ''!r} "
            f"ms={res.elapsed_ms} err={res.error or ''}"
        )
        if delay > 0 and i != args.requests - 1:
            time.sleep(delay + random.uniform(0, args.jitter))
    return out


def summarize(results: Iterable[Result]) -> dict[str, object]:
    rows = list(results)
    lat = [r.elapsed_ms for r in rows if r.elapsed_ms is not None]
    statuses: dict[str, int] = {}
    reasons: dict[str, int] = {}
    for r in rows:
        statuses[str(r.status_code)] = statuses.get(str(r.status_code), 0) + 1
        if r.block_reason:
            reasons[r.block_reason] = reasons.get(r.block_reason, 0) + 1
    return {
        "requests": len(rows),
        "ok": sum(1 for r in rows if r.ok),
        "blocked": sum(1 for r in rows if r.blocked),
        "errors": sum(1 for r in rows if r.error),
        "statuses": statuses,
        "block_reasons": reasons,
        "latency_ms_avg": round(statistics.mean(lat), 1) if lat else None,
        "latency_ms_p50": round(statistics.median(lat), 1) if lat else None,
        "latency_ms_max": max(lat) if lat else None,
    }


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--terms", default=",".join(DEFAULT_TERMS), help="Comma-separated search terms")
    p.add_argument("--requests", type=int, default=5, help="Requests per scenario")
    p.add_argument("--delays", default="0.35,1.5", help="Comma-separated seconds between requests")
    p.add_argument("--session-modes", default="shared,isolated", help="Comma-separated: shared,isolated")
    p.add_argument("--endpoint-types", default="api,html", help="Comma-separated: api,html")
    p.add_argument("--timeout", type=float, default=35.0)
    p.add_argument("--jitter", type=float, default=0.2)
    p.add_argument("--rotate-user-agent", action="store_true", help="Rotate UA labels within scenarios")
    p.add_argument("--direct", action="store_true", help="Also test direct/no-proxy access")
    p.add_argument("--output", default="", help="Output JSONL path; default data/benchmarks/dsp_access_<ts>.jsonl")
    return p.parse_args()


def main() -> int:
    args = parse_args()
    terms = [t.strip() for t in args.terms.split(",") if t.strip()]
    delays = [float(d.strip()) for d in args.delays.split(",") if d.strip()]
    session_modes = [s.strip() for s in args.session_modes.split(",") if s.strip()]
    endpoint_types = [e.strip() for e in args.endpoint_types.split(",") if e.strip()]
    for m in session_modes:
        if m not in {"shared", "isolated"}:
            raise SystemExit(f"Unsupported session mode: {m}")
    for e in endpoint_types:
        if e not in {"api", "html"}:
            raise SystemExit(f"Unsupported endpoint type: {e}")

    proxies = proxy_urls()
    if args.direct:
        proxies = [None] + proxies
    if not proxies:
        raise SystemExit("No proxy configured. Set PACKETSTREAM_PROXY_URLS or CRAWLEX_PROXY_* in .env, or use --direct.")

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    out_path = Path(args.output) if args.output else OUT_DIR / f"dsp_access_{int(time.time())}.jsonl"
    all_results: list[Result] = []
    print("proxy_count", len([p for p in proxies if p]), "direct", any(p is None for p in proxies))
    print("terms", ",".join(terms))
    print("output", out_path)

    for proxy_idx, proxy_url in enumerate(proxies):
        for endpoint_type in endpoint_types:
            for session_mode in session_modes:
                for delay in delays:
                    scenario = f"p{proxy_idx}-{endpoint_type}-{session_mode}-delay{delay}"
                    all_results.extend(run_scenario(args, scenario, session_mode, endpoint_type, terms, delay, proxy_url))

    with out_path.open("w") as f:
        for r in all_results:
            f.write(json.dumps(asdict(r), ensure_ascii=False, sort_keys=True) + "\n")

    print("\nSUMMARY")
    print(json.dumps(summarize(all_results), ensure_ascii=False, indent=2, sort_keys=True))
    by_scenario: dict[str, list[Result]] = {}
    for r in all_results:
        by_scenario.setdefault(r.scenario, []).append(r)
    print("\nBY_SCENARIO")
    for scenario, rows in sorted(by_scenario.items()):
        print(scenario, json.dumps(summarize(rows), ensure_ascii=False, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
