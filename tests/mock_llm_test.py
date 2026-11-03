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

    def test_429once_refuses_the_first_time_and_succeeds_after(self):
        _, first = self.srv.post("unique 429once marker A")
        self.assertEqual(first.status, 429)
        self.assertEqual(first.getheader("retry-after"), "1")
        first.read()
        _, second = self.srv.post("unique 429once marker A")
        self.assertEqual(second.status, 200)
        self.assertIn("[DONE]", second.read().decode())
        # A different message is refused on its own first try.
        _, other = self.srv.post("unique 429once marker B")
        self.assertEqual(other.status, 429)

    def test_modes_can_be_chosen_by_the_model_name(self):
        for model, status in (("m-429", 429), ("m-fail", 401), ("m-500", 500), ("m", 200)):
            _, r = self.srv.post("a perfectly ordinary message", extra={"model": model})
            self.assertEqual(r.status, status, model)
            r.read()

    def test_reset_sends_two_deltas_then_aborts_the_connection(self):
        conn, r = self.srv.post("please reset")
        self.assertEqual(r.status, 200)
        got = b""
        with self.assertRaises((ConnectionError, http.client.IncompleteRead)):
            while True:
                chunk = r.read(64)
                if not chunk:
                    break
                got += chunk
            raise http.client.IncompleteRead(got)  # a clean end would not be a reset
        self.assertNotIn(b"[DONE]", got)

    def test_hang_sends_nothing_until_client_closes(self):
        c = http.client.HTTPConnection("127.0.0.1", self.srv.port, timeout=1)
        c.request("POST", "/v1/chat/completions", json.dumps(
            {"model": "m", "stream": True, "messages": [{"role": "user", "content": "hang"}]}),
            {"content-type": "application/json"})
        with self.assertRaises(TimeoutError):
            c.getresponse()
        c.close()

    def test_drop_sends_two_deltas_then_closes_without_done(self):
        _, r = self.srv.post("drop")
        self.assertEqual(r.status, 200)
        data = r.read().decode()
        self.assertEqual(data.count('"content"'), 2)
        self.assertNotIn("[DONE]", data)

    def usage_chunk(self, text, extra):
        _, r = self.srv.post(text, extra=extra)
        events = [json.loads(l[6:]) for l in r.read().decode().splitlines()
                  if l.startswith("data: ") and l != "data: [DONE]"]
        return [e for e in events if "usage" in e]

    def test_usage_chunk_when_requested(self):
        u = self.usage_chunk("hello", {"stream_options": {"include_usage": True}})
        self.assertEqual(len(u), 1)
        self.assertEqual(u[0]["choices"], [])
        self.assertEqual(u[0]["usage"]["prompt_tokens"], 100)
        self.assertEqual(u[0]["usage"]["completion_tokens"], 50)
        self.assertEqual(u[0]["usage"]["prompt_tokens_details"]["cached_tokens"], 0)

    def test_usage_chunk_reports_cached_tokens(self):
        u = self.usage_chunk("a cached prompt", {"stream_options": {"include_usage": True}})
        self.assertGreater(u[0]["usage"]["prompt_tokens_details"]["cached_tokens"], 0)

    def test_no_usage_chunk_without_stream_options(self):
        self.assertEqual(self.usage_chunk("hello", None), [])
        self.assertEqual(self.usage_chunk("hello", {"stream_options": {"include_usage": False}}), [])

    def anthropic(self, text, extra=None):
        body = {"model": "m", "stream": True, "max_tokens": 100, "system": "sys",
                "messages": [{"role": "user", "content": [{"type": "text", "text": text}]}]}
        body.update(extra or {})
        c = http.client.HTTPConnection("127.0.0.1", self.srv.port, timeout=10)
        c.request("POST", "/v1/messages", json.dumps(body), {"content-type": "application/json"})
        return c.getresponse()

    def test_anthropic_stream_events(self):
        r = self.anthropic("a cached prompt")
        self.assertEqual(r.status, 200)
        events, data = [], []
        for line in r.read().decode().splitlines():
            if line.startswith("event: "):
                events.append(line[7:])
            elif line.startswith("data: "):
                data.append(json.loads(line[6:]))
        self.assertEqual(events[0], "message_start")
        self.assertEqual(events[-2:], ["message_delta", "message_stop"])
        self.assertIn("content_block_delta", events)
        start = data[0]["message"]["usage"]
        self.assertEqual(start["input_tokens"], 100)
        self.assertEqual(start["cache_read_input_tokens"], 64)
        md = next(d for d in data if d["type"] == "message_delta")
        self.assertEqual(md["usage"]["output_tokens"], 50)

    def test_anthropic_honors_modes(self):
        self.assertEqual(self.anthropic("429").status, 429)
        self.assertEqual(self.anthropic("500").status, 500)
        self.assertEqual(self.anthropic("fail").status, 401)
        r = self.anthropic("drop")
        data = r.read().decode()
        self.assertEqual(data.count("content_block_delta\ndata"), 2)
        self.assertNotIn("message_stop", data)


if __name__ == "__main__":
    unittest.main()
