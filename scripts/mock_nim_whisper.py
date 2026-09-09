"""Mock NIM whisper endpoint for Rust client verification (stdlib only)."""
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

EXPECT_TEXT = "你好世界"


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        if self.path != "/v1/audio/transcriptions":
            body = json.dumps({"detail": "not found"}).encode()
            self.send_response(404)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length) if length > 0 else b""
        ctype = self.headers.get("Content-Type", "")
        # minimal multipart sanity: must carry file bytes + language field
        ok = (
            "multipart/form-data" in ctype
            and b'name="file"' in body
            and b"zh-CN" in body
        )
        if not ok:
            bad = json.dumps({"detail": "bad multipart"}).encode()
            self.send_response(400)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(bad)))
            self.end_headers()
            self.wfile.write(bad)
            return
        good = json.dumps({"text": EXPECT_TEXT}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(good)))
        self.end_headers()
        self.wfile.write(good)


if __name__ == "__main__":
    print("[mock-nim] listening on http://127.0.0.1:18923", flush=True)
    ThreadingHTTPServer(("127.0.0.1", 18923), Handler).serve_forever()
