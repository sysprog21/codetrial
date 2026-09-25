import importlib.util
import json
import unittest
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

    def test_thought_markup_is_removed_from_text(self):
        looped = "<|channel>thought\n<channel|>" * 3
        self.assertEqual(SHIM.to_parts({"content": looped}), [{"text": ""}])
        self.assertEqual(
            SHIM.to_parts({"content": "- A note.\n<|channel>thought\nhalf"}),
            [{"text": "- A note.\n"}],
        )


if __name__ == "__main__":
    unittest.main()
