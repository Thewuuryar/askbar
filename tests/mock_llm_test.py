"""Self-test for tests/mock_llm.py: python3 tests/mock_llm_test.py"""
import http.client
import json
import os
import subprocess
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
MOCK = os.path.join(HERE, "mock_llm.py")


class MockServer:
    def __enter__(self):
        import socket
        s = socket.socket()
        s.bind(("127.0.0.1", 0))
        self.port = s.getsockname()[1]
        s.close()
        self.proc = subprocess.Popen([sys.executable, MOCK, str(self.port)],
                                     stdout=subprocess.PIPE, text=True)
        self.proc.stdout.readline()  # "mock LLM on ..."
        return self

    def __exit__(self, *a):
        self.proc.kill()
        self.proc.wait()

    def post(self, text, path="/v1/chat/completions", extra=None, timeout=10):
        conn = http.client.HTTPConnection("127.0.0.1", self.port, timeout=timeout)
        body = {"model": "m", "stream": True, "messages": [{"role": "user", "content": text}]}
        body.update(extra or {})
        conn.request("POST", path, json.dumps(body), {"content-type": "application/json"})
        return conn, conn.getresponse()


class Modes(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.srv = MockServer().__enter__()

    @classmethod
    def tearDownClass(cls):
        cls.srv.__exit__()

    def test_429(self):
        _, r = self.srv.post("please 429")
        self.assertEqual(r.status, 429)
        self.assertEqual(r.getheader("retry-after"), "1")
        self.assertIn("error", json.loads(r.read()))

    def test_500(self):
        _, r = self.srv.post("please 500")
        self.assertEqual(r.status, 500)


if __name__ == "__main__":
    unittest.main()
