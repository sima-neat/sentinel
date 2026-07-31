#!/usr/bin/env python3
"""Small dependency-free client for the Sentinel Unix-socket API."""

import argparse
import json
import socket
import sys
from urllib.parse import quote


def request(socket_path, method, path, payload=None):
    body = b"" if payload is None else json.dumps(payload).encode("utf-8")
    headers = [
        f"{method} {path} HTTP/1.1",
        "Host: localhost",
        "Accept: application/json",
        "Connection: close",
        f"Content-Length: {len(body)}",
    ]
    if body:
        headers.append("Content-Type: application/json")
    wire = ("\r\n".join(headers) + "\r\n\r\n").encode("ascii") + body
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.settimeout(10)
        client.connect(socket_path)
        client.sendall(wire)
        chunks = []
        while True:
            chunk = client.recv(65536)
            if not chunk:
                break
            chunks.append(chunk)
    response = b"".join(chunks)
    header, separator, body = response.partition(b"\r\n\r\n")
    if not separator:
        raise RuntimeError("Sentinel returned an invalid HTTP response")
    status_line = header.splitlines()[0].decode("ascii", errors="replace")
    try:
        status = int(status_line.split()[1])
    except (IndexError, ValueError) as error:
        raise RuntimeError(f"invalid Sentinel status line: {status_line}") from error
    document = json.loads(body or b"{}")
    if status >= 400:
        raise RuntimeError(document.get("error", f"Sentinel API returned HTTP {status}"))
    return document


def build_parser():
    root = argparse.ArgumentParser(description=__doc__)
    root.add_argument("--socket", default="/run/simaai-sentinel/api.sock")
    commands = root.add_subparsers(dest="command", required=True)
    for command in ("health", "latest", "metrics", "active", "stop", "runs"):
        commands.add_parser(command)
    run = commands.add_parser("run")
    run.add_argument("selector")
    compare = commands.add_parser("compare")
    compare.add_argument("selectors", nargs="+", metavar="RUN")
    compare.add_argument("--raw", action="store_true", help="include timestamped samples")
    start = commands.add_parser("start")
    start.add_argument("--name", required=True)
    start.add_argument("--note")
    start.add_argument("--tag", action="append", default=[])
    return root


def main():
    argument_parser = build_parser()
    args = argument_parser.parse_args()
    routes = {
        "health": ("GET", "/v1/health", None),
        "latest": ("GET", "/v1/samples/latest", None),
        "metrics": ("GET", "/v1/metrics", None),
        "active": ("GET", "/v1/traces/active", None),
        "stop": ("POST", "/v1/traces/stop", None),
        "runs": ("GET", "/v1/runs", None),
    }
    if args.command == "run":
        operation = ("GET", f"/v1/runs/{quote(args.selector, safe='')}", None)
    elif args.command == "compare":
        if len(args.selectors) < 2:
            argument_parser.error("compare requires at least two runs")
        encoded = ",".join(quote(value, safe="") for value in args.selectors)
        raw = "&raw=1" if args.raw else ""
        operation = ("GET", f"/v1/compare?runs={encoded}{raw}", None)
    elif args.command == "start":
        operation = (
            "POST",
            "/v1/traces",
            {"name": args.name, "note": args.note, "tags": args.tag},
        )
    else:
        operation = routes[args.command]
    try:
        document = request(args.socket, *operation)
    except (OSError, RuntimeError, json.JSONDecodeError) as error:
        print(f"Sentinel API error: {error}", file=sys.stderr)
        return 1
    print(json.dumps(document, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
