import importlib.util
import json
import threading
import unittest
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "gemini_shim", ROOT / "scripts/gemini-shim.py"
)
SHIM = importlib.util.module_from_spec(SPEC)
assert SPEC.loader
SPEC.loader.exec_module(SHIM)


class ToolTranslationTests(unittest.TestCase):
    def test_declarations_become_function_tools(self):
        tools = SHIM.to_tools(
            {
                "tools": [
                    {
                        "functionDeclarations": [
                            {"name": "read_editor", "description": "Read it."},
                            {
                                "name": "log_hint",
                                "description": "Record a hint.",
                                "parameters": {
                                    "type": "OBJECT",
                                    "properties": {"requested": {"type": "BOOLEAN"}},
                                    "required": ["requested"],
                                },
                            },
                        ]
                    }
                ]
            }
        )
        self.assertEqual(
            [tool["function"]["name"] for tool in tools], ["read_editor", "log_hint"]
        )
        self.assertEqual(
            tools[0]["function"]["parameters"], {"type": "object", "properties": {}}
        )
        self.assertEqual(
            tools[1]["function"]["parameters"]["properties"]["requested"],
            {"type": "boolean"},
        )

    def test_a_call_and_its_answer_share_an_id(self):
        messages = SHIM.to_messages(
            [
                {"role": "user", "parts": [{"text": "Hint?"}]},
                {
                    "role": "model",
                    "parts": [
                        {
                            "functionCall": {
                                "name": "log_hint",
                                "args": {"requested": True},
                            }
                        }
                    ],
                },
                {
                    "role": "user",
                    "parts": [
                        {
                            "functionResponse": {
                                "name": "log_hint",
                                "response": {"result": "clue"},
                            }
                        }
                    ],
                },
            ]
        )
        self.assertEqual([m["role"] for m in messages], ["user", "assistant", "tool"])
        call = messages[1]["tool_calls"][0]
        self.assertEqual(json.loads(call["function"]["arguments"]), {"requested": True})
        self.assertEqual(messages[2]["tool_call_id"], call["id"])
        self.assertEqual(json.loads(messages[2]["content"]), {"result": "clue"})

    def test_two_calls_to_one_tool_are_answered_in_order(self):
        messages = SHIM.to_messages(
            [
                {
                    "role": "model",
                    "parts": [
                        {"functionCall": {"name": "read_editor"}},
                        {"functionCall": {"name": "read_editor"}},
                    ],
                },
                {
                    "role": "user",
                    "parts": [
                        {
                            "functionResponse": {
                                "name": "read_editor",
                                "response": {"n": 1},
                            }
                        },
                        {
                            "functionResponse": {
                                "name": "read_editor",
                                "response": {"n": 2},
                            }
                        },
                    ],
                },
            ]
        )
        ids = [call["id"] for call in messages[0]["tool_calls"]]
        self.assertEqual(len(set(ids)), 2)
        self.assertEqual([m["tool_call_id"] for m in messages[1:]], ids)

    def test_a_reply_with_calls_becomes_function_call_parts(self):
        parts = SHIM.to_parts(
            {
                "content": "",
                "tool_calls": [
                    {
                        "id": "abc",
                        "function": {
                            "name": "log_hint",
                            "arguments": '{"requested":true}',
                        },
                    },
                    {
                        "id": "def",
                        "function": {"name": "read_editor", "arguments": "not json"},
                    },
                ],
            }
        )
        self.assertEqual(
            parts,
            [
                {
                    "functionCall": {
                        "id": "abc",
                        "name": "log_hint",
                        "args": {"requested": True},
                    }
                },
                {"functionCall": {"id": "def", "name": "read_editor", "args": {}}},
            ],
        )

    def test_a_plain_reply_stays_one_text_part(self):
        self.assertEqual(SHIM.to_parts({"content": "Hello."}), [{"text": "Hello."}])

    def test_a_report_request_is_unchanged_by_tools_support(self):
        request = SHIM.to_chat_request(
            {
                "contents": [{"parts": [{"text": "prompt"}]}],
                "generationConfig": {"thinkingConfig": {"thinkingBudget": 0}},
            }
        )
        self.assertNotIn("tools", request)
        self.assertEqual(request["messages"], [{"role": "user", "content": "prompt"}])
        self.assertEqual(request["chat_template_kwargs"], {"enable_thinking": False})
        self.assertEqual(request["stop"], [SHIM.THOUGHT_LOOP])

    def test_thinking_left_on_adds_no_stop(self):
        request = SHIM.to_chat_request({"contents": [{"parts": [{"text": "hi"}]}]})
        self.assertNotIn("stop", request)
        self.assertEqual(request["max_tokens"], SHIM.DEFAULT_MAX_TOKENS)

    def test_thinking_can_be_turned_off_for_every_request(self):
        request = SHIM.to_chat_request(
            {"contents": [{"parts": [{"text": "hi"}]}]}, thinking_off=True
        )
        self.assertEqual(request["chat_template_kwargs"], {"enable_thinking": False})
        self.assertEqual(request["stop"], [SHIM.THOUGHT_LOOP])

    def test_the_loop_stop_leaves_room_for_empty_channels_before_a_reply(self):
        # Two empty channels and then a reply is what gemma writes after a tool
        # response; only a fourth opener in a row is the loop.
        reply = SHIM.THOUGHT_BLOCK * 2 + "Here is a nudge."
        self.assertNotIn(SHIM.THOUGHT_LOOP, reply)
        self.assertIn(SHIM.THOUGHT_LOOP, SHIM.THOUGHT_BLOCK * 4)

    def test_stop_sequences_pass_through(self):
        request = SHIM.to_chat_request(
            {
                "contents": [{"parts": [{"text": "hi"}]}],
                "generationConfig": {"stopSequences": ["END"]},
            }
        )
        self.assertEqual(request["stop"], ["END"])

    def test_json_without_a_schema_is_still_held_to_json(self):
        request = SHIM.to_chat_request(
            {
                "contents": [],
                "generationConfig": {"responseMimeType": "application/json"},
            }
        )
        self.assertEqual(request["response_format"], {"type": "json_object"})
        self.assertNotIn("response_format", SHIM.to_chat_request({"contents": []}))

    def test_the_seed_passes_through(self):
        request = SHIM.to_chat_request(
            {"contents": [], "generationConfig": {"seed": 71}}
        )
        self.assertEqual(request["seed"], 71)
        self.assertNotIn("seed", SHIM.to_chat_request({"contents": []}))

    def test_the_caller_deadline_shortens_the_upstream_wait(self):
        self.assertEqual(SHIM.upstream_deadline({"X-Server-Timeout": "12"}, 45), 12)
        self.assertEqual(SHIM.upstream_deadline({"X-Server-Timeout": "90"}, 45), 45)
        for unusable in ({}, {"X-Server-Timeout": "soon"}, {"X-Server-Timeout": "0"}):
            self.assertEqual(SHIM.upstream_deadline(unusable, 45), 45, unusable)

    def test_thought_markup_is_removed_from_text(self):
        looped = "<|channel>thought\n<channel|>" * 3
        self.assertEqual(SHIM.to_parts({"content": looped}), [{"text": ""}])
        self.assertEqual(
            SHIM.to_parts({"content": "- A note.\n<|channel>thought\nhalf"}),
            [{"text": "- A note.\n"}],
        )


class UpstreamFailureTests(unittest.TestCase):
    def serve(self, handler):
        server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        return f"http://127.0.0.1:{server.server_address[1]}"

    def status_through_shim(self, upstream_body, declared_length):
        class Upstream(BaseHTTPRequestHandler):
            def do_POST(self):
                self.rfile.read(int(self.headers["Content-Length"]))
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(declared_length))
                self.end_headers()
                self.wfile.write(upstream_body)
                self.close_connection = True

            def log_message(self, *args):
                pass

        class Shim(SHIM.Handler):
            llama = self.serve(Upstream)

            def log_message(self, *args):
                pass

        request = urllib.request.Request(
            f"{self.serve(Shim)}/v1beta/models/local:generateContent",
            data=json.dumps({"contents": [{"parts": [{"text": "hi"}]}]}).encode(),
            headers={"Content-Type": "application/json"},
        )
        try:
            with urllib.request.urlopen(request, timeout=10) as response:
                return response.status
        except urllib.error.HTTPError as error:
            return error.code

    def test_a_truncated_answer_is_a_503(self):
        body = b'{"choices": [{"message": {"content": "cut'
        self.assertEqual(self.status_through_shim(body, len(body) + 100), 503)

    def test_an_answer_that_is_not_json_is_a_503(self):
        body = b"<html>502 Bad Gateway</html>"
        self.assertEqual(self.status_through_shim(body, len(body)), 503)


if __name__ == "__main__":
    unittest.main()
