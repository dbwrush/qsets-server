#!/usr/bin/env python3
"""Exercise the QSets HTTP workflow with concurrent generation requests."""

import argparse
import concurrent.futures
import http.cookiejar
import json
import re
import statistics
import time
import urllib.error
import urllib.request


CSRF_PATTERN = re.compile(r'window\.QSETS_CSRF\s*=\s*"([^"]+)"')


def request(opener, url, method="GET", payload=None, csrf=None):
    body = None
    headers = {}
    if payload is not None:
        body = json.dumps(payload).encode()
        headers["content-type"] = "application/json"
    if csrf:
        headers["x-csrf-token"] = csrf
    request_obj = urllib.request.Request(url, data=body, headers=headers, method=method)
    started = time.perf_counter()
    with opener.open(request_obj, timeout=30) as response:
        data = response.read()
    elapsed = time.perf_counter() - started
    return json.loads(data), elapsed


def login_once(base_url, username, password):
    cookies = http.cookiejar.CookieJar()
    opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(cookies))
    with opener.open(f"{base_url}/", timeout=30) as response:
        page = response.read().decode()
    match = CSRF_PATTERN.search(page)
    if not match:
        raise RuntimeError("could not find CSRF token on the home page")
    csrf = match.group(1)
    request(opener, f"{base_url}/api/login", "POST", {"username": username, "password": password}, csrf)
    return opener, csrf


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-url", default="http://127.0.0.1:3010")
    parser.add_argument("--username", default="admin")
    parser.add_argument("--password", default="load-test-password")
    parser.add_argument("--pool-id", type=int, default=None)
    parser.add_argument("--users", type=int, default=100, help="concurrent generation workers")
    parser.add_argument("--requests", type=int, default=1, help="generation requests per worker")
    return parser.parse_args()


def main():
    args = parse_args()
    base_url = args.base_url.rstrip("/")
    opener, csrf = login_once(base_url, args.username, args.password)
    pools, _ = request(opener, f"{base_url}/api/pools")
    pool_id = args.pool_id or pools["pools"][0]["id"]
    books, _ = request(opener, f"{base_url}/api/pools/{pool_id}/books")
    filters = [
        {
            "name": book["name"],
            "start_chapter": book["min_chapter"],
            "end_chapter": book["max_chapter"],
        }
        for book in books["books"]
    ]
    if not filters:
        raise RuntimeError("selected pool has no books")

    jobs = [(worker, attempt) for worker in range(args.users) for attempt in range(args.requests)]

    def generate(job):
        worker, attempt = job
        payload = {
            "pool_id": pool_id,
            "question_type": "standard",
            "count": 20,
            "situation": True,
            "seed": worker * args.requests + attempt,
            "books": filters,
        }
        return request(opener, f"{base_url}/api/generate", "POST", payload, csrf)[1]

    started = time.perf_counter()
    failures = []
    latencies = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.users) as executor:
        futures = [executor.submit(generate, job) for job in jobs]
        for future in concurrent.futures.as_completed(futures):
            try:
                latencies.append(future.result())
            except (OSError, urllib.error.HTTPError, RuntimeError, json.JSONDecodeError) as error:
                failures.append(str(error))
    elapsed = time.perf_counter() - started

    print(f"base_url={base_url}")
    print(f"pool_id={pool_id} workers={args.users} requests={len(jobs)}")
    print(f"elapsed_seconds={elapsed:.3f} throughput_per_second={len(jobs) / elapsed:.2f}")
    if latencies:
        ordered = sorted(latencies)
        p95_index = min(len(ordered) - 1, int(len(ordered) * 0.95))
        print(
            "latency_seconds="
            f"min:{min(latencies):.3f} "
            f"median:{statistics.median(latencies):.3f} "
            f"p95:{ordered[p95_index]:.3f} "
            f"max:{max(latencies):.3f}"
        )
    print(f"successes={len(latencies)} failures={len(failures)}")
    for failure in failures[:5]:
        print(f"failure={failure}")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
