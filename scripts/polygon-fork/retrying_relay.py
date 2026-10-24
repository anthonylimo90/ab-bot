"""A JSON-RPC relay for the fork's upstream node that retries refusals.

Load-balanced public nodes route some requests to backends that refuse
historical state ("unsupported block number") or answer 429; Hardhat does
not retry, and a fork that hits one fails every later call that needs that
state. Run it, then fork from it:

    python3 scripts/polygon-fork/retrying_relay.py https://polygon.drpc.org 8546 &
    FORK_URL=http://127.0.0.1:8546 npm run fork

Standard library only. Listens on 127.0.0.1.
"""

import json
import sys
import time
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

UPSTREAM = sys.argv[1]
PORT = int(sys.argv[2]) if len(sys.argv) > 2 else 8546
ATTEMPTS = 8
RETRYABLE = ("unsupported block number", "rate limit", "too many requests", "timeout")


def retryable(body: bytes) -> bool:
    try:
        answer = json.loads(body)
    except ValueError:
        return True
    answers = answer if isinstance(answer, list) else [answer]
    for item in answers:
        error = item.get("error") if isinstance(item, dict) else None
        if error and any(r in str(error).lower() for r in RETRYABLE):
            return True
    return False


class Relay(BaseHTTPRequestHandler):
    def do_POST(self):
        request = self.rfile.read(int(self.headers.get("content-length", 0)))
        body, status = b"", 502
        for attempt in range(ATTEMPTS):
            try:
                upstream = urllib.request.Request(
                    UPSTREAM,
                    data=request,
                    # Some providers refuse urllib's default agent.
                    headers={"content-type": "application/json", "user-agent": "ab-bot-fork-relay"},
                )
                with urllib.request.urlopen(upstream, timeout=30) as response:
                    body, status = response.read(), response.status
                if not retryable(body):
                    break
            except urllib.error.HTTPError as error:
                body, status = error.read(), error.code
                if status not in (429, 500, 502, 503, 504):
                    break
            except (urllib.error.URLError, TimeoutError) as error:
                body, status = json.dumps({"error": str(error)}).encode(), 502
            time.sleep(min(0.25 * 2**attempt, 4))
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


if __name__ == "__main__":
    ThreadingHTTPServer(("127.0.0.1", PORT), Relay).serve_forever()
