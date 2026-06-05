#!/usr/bin/env python3
"""Run Crawlex access-profile experiments.

Examples:

  .venv-meds/bin/python scripts/run_access_profile_experiment.py \
    --profiles dsp-vtex-default-sticky \
    --endpoint-type vtex_api \
    --terms dipirona,paracetamol,losartana \
    --requests 6

  .venv-meds/bin/python scripts/run_access_profile_experiment.py \
    --profiles dsp-vtex-default-sticky,dsp-vtex-default-rotating \
    --endpoint-type ip_check \
    --requests 4
"""

from __future__ import annotations

import argparse
import json
import statistics
import sys
import time
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from crawlex.access_profiles import (  # noqa: E402
    CircuitBreakerOpen,
    RequestProfileClient,
    load_dotenv,
    load_profiles,
    make_headers,
    validate_profile,
)

ENV_FILE = ROOT / ".env"
DEFAULT_CONFIG = ROOT / "config" / "access_profiles.json"
OUT_DIR = ROOT / "data" / "benchmarks"
IP_CHECK_URL = "https://ipv4.icanhazip.com"


def summarize(rows: list[dict[str, Any]]) -> dict[str, Any]:
    latencies = [int(r["elapsed_ms"]) for r in rows if r.get("elapsed_ms") is not None]
    return {
        "requests": len(rows),
        "ok": sum(1 for r in rows if r.get("ok")),
        "blocked": sum(1 for r in rows if r.get("blocked")),
        "errors": sum(1 for r in rows if r.get("error")),
        "statuses": dict(sorted(Counter(str(r.get("status_code")) for r in rows).items())),
        "block_reasons": dict(sorted(Counter(r.get("block_reason") for r in rows if r.get("block_reason")).items())),
        "exit_ips": dict(sorted(Counter(r.get("exit_ip") for r in rows if r.get("exit_ip")).items())),
        "latency_ms_avg": round(statistics.mean(latencies), 1) if latencies else None,
        "latency_ms_p50": round(statistics.median(latencies), 1) if latencies else None,
        "latency_ms_max": max(latencies) if latencies else None,
    }


def endpoint_for_profile(endpoint_type: str, allowed: tuple[str, ...]) -> str:
    if endpoint_type != "auto":
        return endpoint_type
    if "vtex_api" in allowed:
        return "vtex_api"
    return allowed[0]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", default=str(DEFAULT_CONFIG))
    parser.add_argument("--profiles", required=True, help="Comma-separated profile names")
    parser.add_argument("--endpoint-type", default="auto", choices=["auto", "vtex_api", "html", "ip_check"])
    parser.add_argument("--terms", default="dipirona,paracetamol,losartana,ibuprofeno,tadalafila")
    parser.add_argument("--requests", type=int, default=5)
    parser.add_argument("--timeout", type=float, default=35.0)
    parser.add_argument("--no-env", action="store_true")
    parser.add_argument("--audit", action="store_true", help="Print profile consistency warnings and generated headers")
    args = parser.parse_args()

    if not args.no_env:
        load_dotenv(ENV_FILE)

    profiles = load_profiles(Path(args.config))
    selected = [name.strip() for name in args.profiles.split(",") if name.strip()]
    terms = [term.strip() for term in args.terms.split(",") if term.strip()]
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    out_path = OUT_DIR / f"access_profiles_{int(time.time())}.jsonl"
    rows: list[dict[str, Any]] = []

    print("output", out_path)
    print("profiles", ",".join(selected))
    print("endpoint_type", args.endpoint_type)

    with out_path.open("w") as fh:
        for profile_name in selected:
            if profile_name not in profiles:
                raise SystemExit(f"unknown profile: {profile_name}")
            profile = profiles[profile_name]
            warnings = validate_profile(profile)
            if args.audit:
                print("\nAUDIT", profile_name)
                print("warnings", json.dumps(warnings, ensure_ascii=False))
                audit_endpoint = endpoint_for_profile(args.endpoint_type, profile.allowed_endpoint_types)  # type: ignore[arg-type]
                if audit_endpoint == "ip_check":
                    audit_endpoint = "vtex_api"
                print("headers", json.dumps(make_headers(profile, audit_endpoint), ensure_ascii=False, sort_keys=True))  # type: ignore[arg-type]
            client = RequestProfileClient(profile)
            effective_endpoint = endpoint_for_profile(args.endpoint_type, profile.allowed_endpoint_types)  # type: ignore[arg-type]
            for i in range(args.requests):
                term = terms[i % len(terms)] if terms else "dipirona"
                try:
                    if effective_endpoint == "ip_check":
                        response = client.fetch_url(IP_CHECK_URL, "ip_check", timeout=args.timeout)
                    else:
                        response = client.fetch_term(term, effective_endpoint, timeout=args.timeout)  # type: ignore[arg-type]
                    row = response.to_json()
                    row["term"] = term if effective_endpoint != "ip_check" else None
                except CircuitBreakerOpen as exc:
                    row = {
                        "profile": profile_name,
                        "endpoint_type": effective_endpoint,
                        "ok": False,
                        "blocked": True,
                        "block_reason": "circuit_breaker_open",
                        "status_code": None,
                        "elapsed_ms": 0,
                        "bytes": 0,
                        "error": str(exc),
                        "term": term,
                    }
                row["ts"] = time.time()
                row["hardware_profile"] = profile.hardware.name
                row["device_type"] = profile.hardware.device_type
                row["os"] = profile.hardware.os
                row["browser"] = profile.hardware.browser
                row["session_mode"] = profile.session.mode
                row["proxy_mode"] = profile.proxy.mode
                row["identity_pool"] = profile.identity_pool
                row["identity_role"] = profile.identity_role
                row["identity_weight"] = profile.identity_weight
                row["sticky_identity_id"] = profile.sticky_identity_id
                row["profile_warnings"] = warnings
                rows.append(row)
                fh.write(json.dumps(row, ensure_ascii=False, sort_keys=True) + "\n")
                fh.flush()
                print(
                    f"{profile_name} #{i+1}/{args.requests} endpoint={effective_endpoint} "
                    f"term={row.get('term') or ''} status={row.get('status_code')} ok={row.get('ok')} "
                    f"blocked={row.get('blocked')} reason={row.get('block_reason')} "
                    f"items={row.get('item_count')} exit_ip={row.get('exit_ip') or ''} "
                    f"ms={row.get('elapsed_ms')} err={row.get('error') or ''}"
                )

    print("\nSUMMARY")
    print(json.dumps(summarize(rows), ensure_ascii=False, indent=2, sort_keys=True))
    by_profile: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for row in rows:
        by_profile[str(row.get("profile"))].append(row)
    print("\nBY_PROFILE")
    for profile_name, profile_rows in sorted(by_profile.items()):
        print(profile_name, json.dumps(summarize(profile_rows), ensure_ascii=False, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
