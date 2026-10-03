#!/usr/bin/env python3
"""Answer Gemini `generateContent` calls from a local llama.cpp server.

CodeTrial's report and interim-review calls, and the text interviewer in
`tests/interview_behavior.rs`, speak Gemini's REST API. This shim accepts that
envelope, forwards it to llama-server's OpenAI-compatible
`/v1/chat/completions`, and answers in Gemini's shape, so the Rust side only
needs `CODETRIAL_GEMINI_REST_BASE` pointed here. Function declarations, calls
and responses are carried across as OpenAI tools, so run llama-server with
`--jinja`. The live interviewer socket is not handled; it still goes to Google.

    llama-server -m model.gguf --port 8080 -ngl 99 -c 32768
    scripts/gemini-shim.py --listen 127.0.0.1:8090 --llama http://127.0.0.1:8080 \
        --thinking off
    CODETRIAL_GEMINI_REST_BASE=http://127.0.0.1:8090 make web

Standard library only, so it runs wherever the test gate's Python does.
"""

import argparse
import json
import re
import sys
import time
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROUTE = re.compile(r"^/v1beta/models/([^/:]+):generateContent$")

# How long one request may hold llama-server. The Rust side gives a local report
# attempt 45 seconds (LOCAL_REPORT_ATTEMPT_TIMEOUT) and then retries, but the
# shim does not see it leave, so a longer wait here kept the GPU generating an
# answer nobody would read while the retry queued behind it. Closing the
# upstream socket is what stops llama-server, within about two seconds, so the
# default matches that deadline; --upstream-timeout changes it.
UPSTREAM_TIMEOUT_S = 45

# For a request that names no output limit. llama-server's own default is none,
# so a model that falls into repeating itself writes until the context is full:
# an interviewer turn in the behaviour check ran past 9,900 tokens. With a limit
# that turn ends as MAX_TOKENS instead, which the caller sees and a hang hides.
# Thinking counts against it too.
#
# Sized to finish inside UPSTREAM_TIMEOUT_S, or the limit never arrives: gemma-4-
# 12b on a 5070 Ti writes about 77 tokens a second, so 4096 took 53 seconds and
# would now end as a timeout. 2048 takes about 27. The longest turn seen with
# thinking off was 93 tokens, and with it on the reasoning ran to about 300.
DEFAULT_MAX_TOKENS = 2048

# Gemma 4's thought-channel markup. With thinking turned off, a model that has
# nothing to say opens an empty thought channel instead of stopping, closes it
# and opens another, until the output limit: the interim review's "return
# nothing" case spent 512 tokens and seven seconds that way, and the markup came
# back as three notes.
#
# Not a stop at the first opener, which is what this used to be: after a tool
# response the model writes one or two empty channels and then its real reply,
# and stopping at the first one returned 27 of 28 such turns empty. Replayed
# against those turns, a stop at the fourth consecutive opener let all 28 reply
# and still ended the empty case in about 13 tokens; at the third, 2 of 28 were
# cut off. Whatever markup reaches the text is taken back out.
THOUGHT_BLOCK = "<|channel>thought\n<channel|>"
THOUGHT_LOOP = "<channel|>" + THOUGHT_BLOCK * 2 + "<|channel>"
THOUGHT_MARKUP = re.compile(r"<\|channel>.*?(?:<channel\|>|$)|<channel\|>", re.S)


def convert_schema(node):
    """Gemini's OpenAPI subset to the JSON Schema llama.cpp compiles to a grammar.

    Types are upper-case enum names there and lower-case here, `nullable`
    becomes an `anyOf` with null, and `propertyOrdering` becomes the order of
    `properties`, which is the order the grammar emits keys in. Objects are
    closed, so the model cannot pad a report with fields nobody reads.
    """
    if isinstance(node, list):
        return [convert_schema(item) for item in node]
    if not isinstance(node, dict):
        return node
    out = {}
    for key, value in node.items():
        if key in ("propertyOrdering", "nullable"):
            continue
        if key == "type" and isinstance(value, str):
            out["type"] = value.lower()
        elif key == "properties":
            order = node.get("propertyOrdering") or []
            names = [n for n in order if n in value] + [
                n for n in value if n not in order
            ]
            out["properties"] = {n: convert_schema(value[n]) for n in names}
        else:
            out[key] = convert_schema(value)
    if out.get("type") == "object":
        out.setdefault("additionalProperties", False)
    if node.get("nullable"):
        return {"anyOf": [out, {"type": "null"}]}
    return out


def parts_text(content):
    return "".join(part.get("text", "") for part in content.get("parts", []))


def to_tools(body):
    """Gemini `functionDeclarations` as OpenAI function tools.

    A declaration without parameters still gets an empty object schema, which
    is what a chat template expects to render.
    """
    tools = []
    for group in body.get("tools", []):
        for declaration in group.get("functionDeclarations", []):
            parameters = declaration.get("parameters")
            tools.append(
                {
                    "type": "function",
                    "function": {
                        "name": declaration["name"],
                        "description": declaration.get("description", ""),
                        "parameters": convert_schema(parameters)
                        if parameters
                        else {"type": "object", "properties": {}},
                    },
                }
            )
    return tools


def to_messages(contents):
    """Gemini turns as OpenAI messages, function calls and responses included.

    Gemini pairs a `functionResponse` with its call by name and order, where
    OpenAI pairs them by id. A call that arrives without an id gets one here,
    and each response takes the id of the oldest unanswered call of the same
    name, so two calls to one tool in a turn are answered in the order made.
    """
    messages = []
    unanswered = []
    for turn, content in enumerate(contents):
        parts = content.get("parts", [])
        text = "".join(part.get("text", "") for part in parts)
        calls = [part["functionCall"] for part in parts if "functionCall" in part]
        answers = [
            part["functionResponse"] for part in parts if "functionResponse" in part
        ]
        if content.get("role") == "model":
            message = {"role": "assistant", "content": text}
            if calls:
                message["tool_calls"] = []
                for index, call in enumerate(calls):
                    call_id = call.get("id") or f"call_{turn}_{index}"
                    unanswered.append((call_id, call["name"]))
                    message["tool_calls"].append(
                        {
                            "id": call_id,
                            "type": "function",
                            "function": {
                                "name": call["name"],
                                "arguments": json.dumps(call.get("args", {})),
                            },
                        }
                    )
            messages.append(message)
            continue
        for answer in answers:
            match = next(
                (pending for pending in unanswered if pending[0] == answer.get("id")),
                None,
            ) or next(
                (pending for pending in unanswered if pending[1] == answer["name"]),
                None,
            )
            if match:
                unanswered.remove(match)
            messages.append(
                {
                    "role": "tool",
                    "tool_call_id": match[0] if match else f"call_{turn}_orphan",
                    "content": json.dumps(answer.get("response", {})),
                }
            )
        if text or not answers:
            messages.append({"role": "user", "content": text})
    return messages


def to_chat_request(body, thinking_off=False):
    messages = []
    system = body.get("systemInstruction")
    if system:
        messages.append({"role": "system", "content": parts_text(system)})
    messages.extend(to_messages(body.get("contents", [])))

    config = body.get("generationConfig", {})
    request = {"messages": messages, "stream": False}
    tools = to_tools(body)
    if tools:
        request["tools"] = tools
    if "temperature" in config:
        request["temperature"] = config["temperature"]
    request["max_tokens"] = config.get("maxOutputTokens", DEFAULT_MAX_TOKENS)
    if (
        config.get("responseMimeType") == "application/json"
        and "responseSchema" in config
    ):
        request["response_format"] = {
            "type": "json_schema",
            "json_schema": {
                "name": "response",
                "strict": True,
                "schema": convert_schema(config["responseSchema"]),
            },
        }

    # The Rust side asks for no thinking because Gemini charges thinking tokens
    # against maxOutputTokens; a reasoning model here does the same, so honour it.
    stop = list(config.get("stopSequences", []))
    thinking = config.get("thinkingConfig", {})
    if (
        thinking_off
        or thinking.get("thinkingBudget") == 0
        or thinking.get("thinkingLevel") == "NONE"
    ):
        request["chat_template_kwargs"] = {"enable_thinking": False}
        stop.append(THOUGHT_LOOP)
    if stop:
        request["stop"] = stop
    return request


FINISH_REASONS = {"stop": "STOP", "tool_calls": "STOP", "length": "MAX_TOKENS"}


def to_parts(message):
    """An OpenAI assistant message as Gemini parts: its text, then its calls.

    Arguments arrive as a JSON string and leave as an object. Ones that do not
    parse are passed on empty rather than failing the turn, which is what the
    caller would see from a model that called the tool with nothing.
    """
    parts = []
    text = THOUGHT_MARKUP.sub("", message.get("content") or "")
    calls = message.get("tool_calls") or []
    if text or not calls:
        parts.append({"text": text})
    for call in calls:
        function = call.get("function", {})
        try:
            args = json.loads(function.get("arguments") or "{}")
        except json.JSONDecodeError:
            args = {}
        parts.append(
            {
                "functionCall": {
                    "id": call.get("id", ""),
                    "name": function.get("name", ""),
                    "args": args if isinstance(args, dict) else {},
                }
            }
        )
    return parts


def to_gemini_response(chat):
    choice = (chat.get("choices") or [{}])[0]
    usage = chat.get("usage") or {}
    return {
        "candidates": [
            {
                "content": {
                    "role": "model",
                    "parts": to_parts(choice.get("message") or {}),
                },
                "finishReason": FINISH_REASONS.get(
                    choice.get("finish_reason"), "OTHER"
                ),
            }
        ],
        "usageMetadata": {
            "promptTokenCount": usage.get("prompt_tokens", 0),
            "candidatesTokenCount": usage.get("completion_tokens", 0),
            "totalTokenCount": usage.get("total_tokens", 0),
        },
        "modelVersion": chat.get("model", "local"),
    }


class Handler(BaseHTTPRequestHandler):
    llama = "http://127.0.0.1:8080"
    upstream_timeout = UPSTREAM_TIMEOUT_S
    thinking_off = False

    def do_POST(self):
        match = ROUTE.match(self.path.split("?", 1)[0])
        if not match:
            self.reply(404, error_body(404, f"no route for {self.path}"))
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
            body = json.loads(self.rfile.read(length) or b"{}")
        except (ValueError, json.JSONDecodeError) as error:
            self.reply(400, error_body(400, f"bad request body: {error}"))
            return

        request = to_chat_request(body, self.thinking_off)
        started = time.monotonic()
        upstream = urllib.request.Request(
            f"{self.llama}/v1/chat/completions",
            data=json.dumps(request).encode(),
            headers={"Content-Type": "application/json"},
        )
        try:
            with urllib.request.urlopen(
                upstream, timeout=self.upstream_timeout
            ) as response:
                chat = json.load(response)
        except urllib.error.HTTPError as error:
            # Status passes through so the Rust retry rules see a 503 as a 503.
            detail = error.read().decode(errors="replace")[:500]
            self.log_message("upstream %d: %s", error.code, detail)
            self.reply(error.code, error_body(error.code, detail))
            return
        except (urllib.error.URLError, TimeoutError, ConnectionError) as error:
            self.log_message("upstream unreachable: %s", error)
            self.reply(503, error_body(503, f"llama-server unreachable: {error}"))
            return

        answer = to_gemini_response(chat)
        meta = answer["usageMetadata"]
        self.log_message(
            "%s -> %s: %d in, %d out, %s, %.1fs",
            match.group(1),
            answer["modelVersion"],
            meta["promptTokenCount"],
            meta["candidatesTokenCount"],
            answer["candidates"][0]["finishReason"],
            time.monotonic() - started,
        )
        self.reply(200, answer)

    def reply(self, status, payload):
        data = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, format, *args):
        sys.stderr.write(f"[gemini-shim] {format % args}\n")


def error_body(code, message):
    return {
        "error": {
            "code": code,
            "message": message,
            "status": "UNAVAILABLE" if code == 503 else "ERROR",
        }
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--listen", default="127.0.0.1:8090", help="host:port to serve on"
    )
    parser.add_argument(
        "--llama", default="http://127.0.0.1:8080", help="llama-server base URL"
    )
    parser.add_argument(
        "--upstream-timeout",
        type=float,
        default=UPSTREAM_TIMEOUT_S,
        help="seconds one request may hold llama-server (default: %(default)s)",
    )
    parser.add_argument(
        "--thinking",
        choices=["request", "off"],
        default="request",
        help="follow each request's thinkingConfig, or turn thinking off for all;"
        " a request that names none leaves a reasoning model thinking",
    )
    args = parser.parse_args()

    host, _, port = args.listen.rpartition(":")
    Handler.llama = args.llama.rstrip("/")
    Handler.upstream_timeout = args.upstream_timeout
    Handler.thinking_off = args.thinking == "off"
    server = ThreadingHTTPServer((host or "127.0.0.1", int(port)), Handler)
    sys.stderr.write(f"[gemini-shim] {args.listen} -> {Handler.llama}\n")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
