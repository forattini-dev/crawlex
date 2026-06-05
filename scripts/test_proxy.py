#!/usr/bin/env python3
"""Test Crawlex proxy settings without printing credentials.

Loads proxy configuration from .env by default and performs:
1. exit IP check against CRAWLEX_PROXY_TEST_URL
2. optional crawler-target smoke request against CRAWLEX_PROXY_TARGET_URL

Never prints the proxy URL or credentials.
"""

from __future__ import annotations

import os
import re
import sys
from pathlib import Path
from urllib.parse import quote

import requests

ROOT = Path(__file__).resolve().parents[1]
ENV_FILE = Path(os.environ.get("CRAWLEX_ENV_FILE", ROOT / ".env"))

SECRET_PATTERNS = [
    re.compile(r"(socks5h?://)[^\s/@:]+:[^\s/@]+@", re.I),
    re.compile(r"(CRAWLEX_PROXY_PASSWORD=).*", re.I),
    re.compile(r"(CRAWLEX_PROXY_USER=).*", re.I),
]


def redact(value: object) -> str:
    text = str(value)
    for pattern in SECRET_PATTERNS:
        text = pattern.sub(r"\1[REDACTED]@" if "socks" in pattern.pattern else r"\1[REDACTED]", text)
    return text


def load_env(path: Path) -> None:
    if not path.exists():
        raise SystemExit(f"Missing env file: {path}. Copy .env.example to .env and fill credentials.")
    for raw_line in path.read_text().splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        key = key.strip()
        value = value.strip().strip('"').strip("'")
        os.environ.setdefault(key, value)


def require_env(name: str) -> str:
    value = os.environ.get(name, "")
    if not value:
        raise SystemExit(f"Missing required env var: {name}")
    return value


def build_proxy_url() -> str:
    scheme = os.environ.get("CRAWLEX_PROXY_SCHEME", "socks5h")
    host = require_env("CRAWLEX_PROXY_HOST")
    port = require_env("CRAWLEX_PROXY_PORT")
    user = quote(require_env("CRAWLEX_PROXY_USER"), safe="")
    password = quote(require_env("CRAWLEX_PROXY_PASSWORD"), safe="")
    return f"{scheme}://{user}:{password}@{host}:{port}"


def fetch(session: requests.Session, url: str) -> requests.Response:
    timeout = float(os.environ.get("CRAWLEX_PROXY_TIMEOUT", "35"))
    return session.get(url, timeout=timeout, allow_redirects=True)


def main() -> int:
    load_env(ENV_FILE)
    proxy_url = build_proxy_url()
    proxies = {"http": proxy_url, "https": proxy_url}

    session = requests.Session()
    session.proxies.update(proxies)
    session.headers.update({
        "User-Agent": os.environ.get(
            "CRAWLEX_USER_AGENT",
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 "
            "(KHTML, like Gecko) Chrome/124.0 Safari/537.36",
        )
    })

    test_url = os.environ.get("CRAWLEX_PROXY_TEST_URL", "https://ipv4.icanhazip.com")
    target_url = os.environ.get("CRAWLEX_PROXY_TARGET_URL", "")

    print(f"env_file={ENV_FILE}")
    print(f"proxy_provider=PacketStream host={os.environ.get('CRAWLEX_PROXY_HOST')} port={os.environ.get('CRAWLEX_PROXY_PORT')}")
    print(f"proxy_scheme={os.environ.get('CRAWLEX_PROXY_SCHEME', 'socks5h')}")

    try:
        r = fetch(session, test_url)
        print(f"exit_ip_check status={r.status_code} url={test_url}")
        print(f"exit_ip={r.text.strip()[:120]}")
        r.raise_for_status()
    except Exception as exc:
        print(f"exit_ip_check error={redact(exc)}", file=sys.stderr)
        return 1

    if target_url:
        try:
            r = fetch(session, target_url)
            title_match = re.search(r"<title[^>]*>(.*?)</title>", r.text, re.I | re.S)
            title = re.sub(r"\s+", " ", title_match.group(1)).strip() if title_match else ""
            print(f"target_smoke status={r.status_code} final_url={r.url}")
            print(f"target_smoke bytes={len(r.content)} title={title[:160]!r}")
        except Exception as exc:
            print(f"target_smoke error={redact(exc)}", file=sys.stderr)
            return 2

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
