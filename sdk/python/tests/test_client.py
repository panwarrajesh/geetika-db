import json
import sys
import threading
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from rajeshdb import Client, RajeshDBError  # noqa: E402

PID = "b" * 32
KEY = "rdb_testkey"


class Fake:
    """A scripted fake server: returns queued (status, body) pairs and records every request."""

    def __init__(self, script):
        self.script = list(script)
        self.seen = []
        outer = self

        class H(BaseHTTPRequestHandler):
            def _do(self):
                n = int(self.headers.get("Content-Length") or 0)
                raw = self.rfile.read(n).decode() if n else ""
                outer.seen.append({"method": self.command, "url": self.path, "auth": self.headers.get("Authorization"),
                                   "body": json.loads(raw) if raw else None})
                status, body = outer.script.pop(0) if outer.script else (500, {"error": {"code": "script_exhausted", "message": "x"}})
                payload = b"" if body is None else json.dumps(body).encode()
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

            do_GET = do_POST = do_PUT = do_DELETE = _do

            def log_message(self, *a):
                pass

        self.server = HTTPServer(("127.0.0.1", 0), H)
        self.url = f"http://127.0.0.1:{self.server.server_port}"
        self.t = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.t.start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()


class ClientTests(unittest.TestCase):
    def test_crud_requests_are_shaped_correctly(self):
        f = Fake([(201, {"_id": "x1"}), (200, {"_id": "x1", "a": 1}), (200, {"_id": "x1"}), (200, {"deleted": True})])
        try:
            c = Client(f.url + "/", KEY, PID, env="staging").collection("users")
            self.assertEqual(c.insert({"a": 1}), "x1")
            self.assertEqual(c.get("x1"), {"_id": "x1", "a": 1})
            c.replace("x1", {"a": 2})
            self.assertTrue(c.delete("x1"))
            base = f"/v1/projects/{PID}/envs/staging/collections/users"
            self.assertEqual([(s["method"], s["url"]) for s in f.seen], [
                ("POST", base + "/documents"), ("GET", base + "/documents/x1"), ("PUT", base + "/documents/x1"), ("DELETE", base + "/documents/x1")])
            self.assertTrue(all(s["auth"] == f"Bearer {KEY}" for s in f.seen))
            self.assertEqual(f.seen[0]["body"], {"a": 1})
        finally:
            f.close()

    def test_404_is_none_or_false_and_other_errors_raise(self):
        err404 = (404, {"error": {"code": "not_found", "message": "gone"}})
        f = Fake([err404, err404, (409, {"error": {"code": "already_exists", "message": "dup"}}), (403, {"error": {"code": "forbidden", "message": "no"}})])
        try:
            c = Client(f.url, KEY, PID).collection("c")
            self.assertIsNone(c.get("nope"))
            self.assertFalse(c.delete("nope"))
            with self.assertRaises(RajeshDBError) as cm:
                c.insert({"_id": "d"})
            self.assertEqual((cm.exception.status, cm.exception.code, cm.exception.message), (409, "already_exists", "dup"))
            with self.assertRaises(RajeshDBError) as cm:
                c.create_index("f")
            self.assertEqual(cm.exception.status, 403)
        finally:
            f.close()

    def test_query_update_delete_where_bodies(self):
        f = Fake([(200, {"documents": [{"_id": "1"}], "count": 1}), (200, {"documents": []}), (200, {"modified": 3}),
                  (400, {"error": {"code": "invalid_argument", "message": "empty"}}), (200, {"deleted": 2})])
        try:
            c = Client(f.url, KEY, PID).collection("c")
            self.assertEqual(c.find(filter={"city": "Delhi"}, sort={"field": "age", "order": "desc"}, limit=5)["count"], 1)
            self.assertIsNone(c.find_one({"x": 1}))
            self.assertEqual(c.update({"a": 1}, {"b": 2}), 3)
            with self.assertRaises(RajeshDBError):
                c.delete_where({})
            self.assertEqual(c.delete_where({}, confirm_all=True), 2)
            self.assertEqual(f.seen[0]["body"], {"filter": {"city": "Delhi"}, "sort": {"field": "age", "order": "desc"}, "limit": 5})
            self.assertEqual(f.seen[1]["body"], {"filter": {"x": 1}, "limit": 1})
            self.assertEqual(f.seen[2]["body"], {"filter": {"a": 1}, "set": {"b": 2}})
            self.assertEqual(f.seen[3]["body"], {"filter": {}})
            self.assertEqual(f.seen[4]["body"], {"filter": {}, "confirm_all": True})
        finally:
            f.close()

    def test_url_encoding_of_ids(self):
        f = Fake([(404, None)])
        try:
            Client(f.url, KEY, PID).collection("c").get("a/b c?d")
            self.assertTrue(f.seen[0]["url"].endswith("/documents/a%2Fb%20c%3Fd"), f.seen[0]["url"])
        finally:
            f.close()

    def test_network_failure_is_a_rajeshdberror(self):
        with self.assertRaises(RajeshDBError) as cm:
            Client("http://127.0.0.1:1", KEY, PID, timeout=2).collection("c").find()
        self.assertEqual((cm.exception.status, cm.exception.code), (0, "network"))

    def test_validation_and_secret_hiding(self):
        with self.assertRaises(ValueError):
            Client("http://x", "nope", PID)
        with self.assertRaises(ValueError):
            Client("http://x", KEY, "short")
        with self.assertRaises(ValueError):
            Client("http://x", KEY, PID, env="qa")
        c = Client("http://x", KEY, PID)
        self.assertNotIn(KEY, repr(c))
        self.assertNotIn(KEY, str(c))


if __name__ == "__main__":
    unittest.main()
