"""FastAPI proxy bridging OpenAI-compatible clients to the Freemodel API."""

from fastapi import FastAPI, Request, HTTPException
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import StreamingResponse, JSONResponse
import getpass
import httpx
import json
import logging
import sys
import time
import uuid

import config
from config import (
    AVAILABLE_MODELS,
    CLIENT_HEADERS,
    DEFAULT_BASE_URL,
    DEFAULT_HOST,
    DEFAULT_PORT,
)

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger("freemodel-proxy")

app = FastAPI(title="Freemodel API Proxy", version="1.1.0")

app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)


def normalize_model(model_name: str | None) -> str:
    """Normalize known model aliases to the canonical Freemodel model name."""
    if not model_name:
        return "gpt-5.6-sol"
    cleaned = model_name.strip().lower()
    aliases = {
        "gpt 5.6 sol",
        "gpt-5.6-sol",
        "gpt-5.6",
        "gpt 5.6",
        "opencode-default",
        "gpt-4o",
        "gpt-4",
    }
    if cleaned in aliases:
        return "gpt-5.6-sol"
    return model_name


def get_auth_token(request: Request) -> str:
    """Extract the Authorization header or fall back to the configured key."""
    auth = request.headers.get("authorization")
    if auth and auth.startswith("Bearer "):
        token = auth.removeprefix("Bearer ").strip()
        if token and token not in {"sk-dummy", "dummy", "placeholder"}:
            return f"Bearer {token}"
    key = config.DEFAULT_API_KEY or config.load_saved_key()
    if key:
        return f"Bearer {key}"
    return ""


def upstream_headers(request: Request) -> dict[str, str]:
    headers = {"Content-Type": "application/json", **CLIENT_HEADERS}
    auth_header = get_auth_token(request)
    if auth_header:
        headers["Authorization"] = auth_header
    return headers


def text_from_content(content) -> str:
    """Flatten Responses/Chat content blocks into text for Chat Completions."""
    if content is None:
        return ""
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        parts = []
        for part in content:
            if isinstance(part, str):
                parts.append(part)
            elif isinstance(part, dict):
                part_type = part.get("type")
                if part_type in {"text", "input_text", "output_text"}:
                    parts.append(str(part.get("text", "")))
                elif part_type == "refusal":
                    parts.append(str(part.get("refusal", "")))
        return "".join(parts)
    return str(content)


def responses_input_to_messages(body: dict) -> list[dict]:
    """Translate Responses API input items to Chat Completions messages."""
    messages: list[dict] = []

    instructions = body.get("instructions")
    if instructions:
        messages.append({"role": "system", "content": text_from_content(instructions)})

    input_value = body.get("input")
    if input_value is None and isinstance(body.get("messages"), list):
        messages.extend(body["messages"])
    elif isinstance(input_value, str):
        messages.append({"role": "user", "content": input_value})
    elif isinstance(input_value, list):
        for item in input_value:
            if isinstance(item, str):
                messages.append({"role": "user", "content": item})
                continue
            if not isinstance(item, dict):
                continue

            item_type = item.get("type", "message")
            if item_type == "message":
                role = item.get("role", "user")
                if role == "developer":
                    role = "system"
                messages.append({"role": role, "content": text_from_content(item.get("content"))})
            elif item_type == "function_call":
                call_id = item.get("call_id") or item.get("id") or f"call_{uuid.uuid4().hex[:16]}"
                arguments = item.get("arguments", "{}")
                if not isinstance(arguments, str):
                    arguments = json.dumps(arguments, separators=(",", ":"))
                messages.append(
                    {
                        "role": "assistant",
                        "content": "",
                        "tool_calls": [
                            {
                                "id": call_id,
                                "type": "function",
                                "function": {
                                    "name": item.get("name", "unknown_tool"),
                                    "arguments": arguments,
                                },
                            }
                        ],
                    }
                )
            elif item_type == "function_call_output":
                messages.append(
                    {
                        "role": "tool",
                        "tool_call_id": item.get("call_id", ""),
                        "content": text_from_content(item.get("output")),
                    }
                )
    elif input_value is None and "prompt" in body:
        messages.append({"role": "user", "content": text_from_content(body.get("prompt"))})

    if not messages:
        messages.append({"role": "user", "content": ""})
    return messages


def responses_tools_to_chat(tools) -> list[dict]:
    """Translate Responses function definitions to Chat Completions tools."""
    translated = []
    if not isinstance(tools, list):
        return translated
    for tool in tools:
        if not isinstance(tool, dict) or tool.get("type") != "function":
            continue
        if isinstance(tool.get("function"), dict):
            translated.append(tool)
            continue
        function = {
            key: tool[key]
            for key in ("name", "description", "parameters", "strict")
            if key in tool
        }
        if function.get("name"):
            translated.append({"type": "function", "function": function})
    return translated


def build_chat_payload(body: dict, model: str) -> dict:
    payload = {
        "model": model,
        "messages": responses_input_to_messages(body),
        "stream": bool(body.get("stream", False)),
    }

    tools = responses_tools_to_chat(body.get("tools"))
    if tools:
        payload["tools"] = tools
        if "tool_choice" in body:
            tool_choice = body["tool_choice"]
            if isinstance(tool_choice, dict) and tool_choice.get("type") == "function":
                tool_choice = {
                    "type": "function",
                    "function": {"name": tool_choice.get("name", "")},
                }
            payload["tool_choice"] = tool_choice
        if "parallel_tool_calls" in body:
            payload["parallel_tool_calls"] = body["parallel_tool_calls"]

    field_map = {
        "max_output_tokens": "max_tokens",
        "temperature": "temperature",
        "top_p": "top_p",
    }
    for source, target in field_map.items():
        if source in body:
            payload[target] = body[source]
    return payload


def convert_usage(usage) -> dict | None:
    if not isinstance(usage, dict):
        return None
    input_tokens = int(usage.get("prompt_tokens", usage.get("input_tokens", 0)) or 0)
    output_tokens = int(usage.get("completion_tokens", usage.get("output_tokens", 0)) or 0)
    total_tokens = int(usage.get("total_tokens", input_tokens + output_tokens) or 0)
    prompt_details = usage.get("prompt_tokens_details") or usage.get("input_tokens_details") or {}
    completion_details = usage.get("completion_tokens_details") or usage.get("output_tokens_details") or {}
    return {
        "input_tokens": input_tokens,
        "input_tokens_details": {
            "cached_tokens": int(prompt_details.get("cached_tokens", 0) or 0),
        },
        "output_tokens": output_tokens,
        "output_tokens_details": {
            "reasoning_tokens": int(completion_details.get("reasoning_tokens", 0) or 0),
        },
        "total_tokens": total_tokens,
    }


def message_output_item(text: str, item_id: str | None = None) -> dict:
    return {
        "id": item_id or f"msg_{uuid.uuid4().hex[:24]}",
        "type": "message",
        "status": "completed",
        "role": "assistant",
        "content": [{"type": "output_text", "text": text, "annotations": []}],
    }


def tool_output_item(tool_call: dict, item_id: str | None = None) -> dict:
    function = tool_call.get("function") if isinstance(tool_call.get("function"), dict) else {}
    arguments = function.get("arguments", "{}")
    if not isinstance(arguments, str):
        arguments = json.dumps(arguments, separators=(",", ":"))
    call_id = tool_call.get("id") or f"call_{uuid.uuid4().hex[:16]}"
    return {
        "id": item_id or f"fc_{uuid.uuid4().hex[:24]}",
        "type": "function_call",
        "status": "completed",
        "name": function.get("name", "unknown_tool"),
        "arguments": arguments,
        "call_id": call_id,
    }


def base_response(response_id: str, model: str, status: str, output: list, usage=None) -> dict:
    response = {
        "id": response_id,
        "object": "response",
        "created_at": int(time.time()),
        "status": status,
        "model": model,
        "output": output,
        "parallel_tool_calls": True,
    }
    converted_usage = convert_usage(usage)
    if converted_usage is not None:
        response["usage"] = converted_usage
    return response


def chat_completion_to_response(chat_result: dict, model: str) -> dict:
    response_id = f"resp_{uuid.uuid4().hex[:24]}"
    choices = chat_result.get("choices") or []
    message = choices[0].get("message", {}) if choices else {}
    output = []
    text = text_from_content(message.get("content"))
    if text or not message.get("tool_calls"):
        output.append(message_output_item(text))
    for tool_call in message.get("tool_calls") or []:
        if isinstance(tool_call, dict):
            output.append(tool_output_item(tool_call))
    return base_response(response_id, model, "completed", output, chat_result.get("usage"))


def sse_event(event_type: str, payload: dict) -> bytes:
    data = {"type": event_type, **payload}
    return f"event: {event_type}\ndata: {json.dumps(data, separators=(',', ':'))}\n\n".encode("utf-8")


def error_json(status_code: int, message: str, error_type: str = "upstream_error") -> JSONResponse:
    return JSONResponse(
        status_code=status_code,
        content={"error": {"message": message, "type": error_type, "code": status_code}},
    )


@app.get("/")
@app.get("/health")
async def health_check():
    return {"status": "ok", "service": "freemodel-proxy", "upstream": config.DEFAULT_BASE_URL}


@app.get("/v1/models")
async def list_models():
    return {"object": "list", "data": AVAILABLE_MODELS}


@app.post("/v1/chat/completions")
async def chat_completions(request: Request):
    try:
        body = await request.json()
    except Exception as exc:
        raise HTTPException(status_code=400, detail=f"Invalid JSON payload: {exc}") from exc

    model = normalize_model(body.get("model"))
    body["model"] = model
    stream = body.get("stream", False)
    target_url = f"{config.DEFAULT_BASE_URL.rstrip('/')}/chat/completions"
    headers = upstream_headers(request)
    logger.info("Proxying chat/completions (model=%s, stream=%s) -> %s", model, stream, target_url)

    if stream:
        async def stream_generator():
            try:
                async with httpx.AsyncClient(timeout=120.0) as client:
                    async with client.stream("POST", target_url, json=body, headers=headers) as response:
                        if response.status_code == 200:
                            async for chunk in response.aiter_bytes():
                                yield chunk
                            return
                        err_str = (await response.aread()).decode("utf-8", errors="ignore")
                        logger.error("Upstream status %s: %s", response.status_code, err_str)
                        err_msg = f"[Upstream Error {response.status_code}]: {err_str}"
            except Exception as exc:
                logger.error("Upstream stream connection error: %s", exc)
                err_msg = f"[Proxy Connection Error]: {exc}"

            completion_id = f"chatcmpl-{uuid.uuid4().hex[:12]}"
            chunk_content = {
                "id": completion_id,
                "object": "chat.completion.chunk",
                "created": int(time.time()),
                "model": model,
                "choices": [{"index": 0, "delta": {"content": err_msg}, "finish_reason": "stop"}],
            }
            yield f"data: {json.dumps(chunk_content)}\n\n".encode("utf-8")
            yield b"data: [DONE]\n\n"

        return StreamingResponse(stream_generator(), media_type="text/event-stream")

    try:
        async with httpx.AsyncClient(timeout=120.0) as client:
            response = await client.post(target_url, json=body, headers=headers)
    except Exception as exc:
        logger.error("Upstream request exception: %s", exc)
        return error_json(502, f"Bad Gateway: {exc}", "proxy_error")

    try:
        return JSONResponse(status_code=response.status_code, content=response.json())
    except Exception:
        return error_json(response.status_code, response.text)


@app.post("/v1/responses")
async def responses_endpoint(request: Request):
    """Adapt the OpenAI Responses API to the upstream Chat Completions API."""
    try:
        body = await request.json()
    except Exception as exc:
        raise HTTPException(status_code=400, detail=f"Invalid JSON payload: {exc}") from exc

    model = normalize_model(body.get("model"))
    chat_payload = build_chat_payload(body, model)
    target_url = f"{config.DEFAULT_BASE_URL.rstrip('/')}/chat/completions"
    headers = upstream_headers(request)
    logger.info("Proxying responses (model=%s, stream=%s) -> %s", model, chat_payload["stream"], target_url)

    if not chat_payload["stream"]:
        try:
            async with httpx.AsyncClient(timeout=120.0) as client:
                response = await client.post(target_url, json=chat_payload, headers=headers)
        except Exception as exc:
            logger.error("Upstream responses request exception: %s", exc)
            return error_json(502, f"Bad Gateway: {exc}", "proxy_error")

        try:
            result = response.json()
        except Exception:
            return error_json(response.status_code, response.text)
        if response.status_code != 200:
            return JSONResponse(status_code=response.status_code, content=result)
        return JSONResponse(content=chat_completion_to_response(result, model))

    client = httpx.AsyncClient(timeout=120.0)
    try:
        upstream_request = client.build_request("POST", target_url, json=chat_payload, headers=headers)
        upstream_response = await client.send(upstream_request, stream=True)
    except Exception as exc:
        await client.aclose()
        logger.error("Upstream responses stream connection error: %s", exc)
        return error_json(502, f"Bad Gateway: {exc}", "proxy_error")

    if upstream_response.status_code != 200:
        raw_error = await upstream_response.aread()
        await upstream_response.aclose()
        await client.aclose()
        error_text = raw_error.decode("utf-8", errors="ignore")
        logger.error("Upstream responses status %s: %s", upstream_response.status_code, error_text)
        try:
            error_content = json.loads(error_text)
        except Exception:
            error_content = {"error": {"message": error_text, "type": "upstream_error"}}
        return JSONResponse(status_code=upstream_response.status_code, content=error_content)

    response_id = f"resp_{uuid.uuid4().hex[:24]}"
    message_id = f"msg_{uuid.uuid4().hex[:24]}"

    async def responses_stream_generator():
        sequence_number = 0
        text_parts: list[str] = []
        tool_calls: dict[int, dict] = {}
        usage = None
        saw_upstream_event = False
        saw_terminal_marker = False
        message_started = False

        def emit(event_type: str, **payload) -> bytes:
            nonlocal sequence_number
            event = sse_event(event_type, {"sequence_number": sequence_number, **payload})
            sequence_number += 1
            return event

        created_response = base_response(response_id, model, "in_progress", [])
        yield emit("response.created", response=created_response)

        try:
            async for line in upstream_response.aiter_lines():
                line = line.strip()
                if not line or not line.startswith("data:"):
                    continue
                data = line[5:].strip()
                if data == "[DONE]":
                    saw_terminal_marker = True
                    break
                try:
                    chunk = json.loads(data)
                except json.JSONDecodeError:
                    logger.warning("Ignoring malformed upstream SSE data: %r", data[:200])
                    continue

                saw_upstream_event = True
                if isinstance(chunk.get("usage"), dict):
                    usage = chunk["usage"]
                for choice in chunk.get("choices") or []:
                    if choice.get("finish_reason") is not None:
                        saw_terminal_marker = True
                    delta = choice.get("delta") or {}
                    content_delta = text_from_content(delta.get("content"))
                    if content_delta:
                        if not message_started:
                            message_started = True
                            partial_item = {
                                "id": message_id,
                                "type": "message",
                                "status": "in_progress",
                                "role": "assistant",
                                "content": [],
                            }
                            yield emit("response.output_item.added", output_index=0, item=partial_item)
                        text_parts.append(content_delta)
                        yield emit(
                            "response.output_text.delta",
                            item_id=message_id,
                            output_index=0,
                            content_index=0,
                            delta=content_delta,
                        )

                    for tool_delta in delta.get("tool_calls") or []:
                        if not isinstance(tool_delta, dict):
                            continue
                        index = int(tool_delta.get("index", 0) or 0)
                        state = tool_calls.setdefault(
                            index,
                            {
                                "id": "",
                                "type": "function",
                                "function": {"name": "", "arguments": ""},
                            },
                        )
                        if tool_delta.get("id"):
                            state["id"] = tool_delta["id"]
                        function_delta = tool_delta.get("function") or {}
                        if function_delta.get("name"):
                            state["function"]["name"] += function_delta["name"]
                        arguments_delta = function_delta.get("arguments")
                        if arguments_delta:
                            state["function"]["arguments"] += arguments_delta

            if not saw_upstream_event or not saw_terminal_marker:
                reason = "Upstream stream ended without a completion marker"
                failed_response = base_response(response_id, model, "failed", [])
                failed_response["error"] = {"code": "upstream_stream_incomplete", "message": reason}
                yield emit("response.failed", response=failed_response)
                return

            output = []
            text = "".join(text_parts)
            if text or not tool_calls:
                item = message_output_item(text, message_id)
                output.append(item)
                yield emit(
                    "response.output_text.done",
                    item_id=message_id,
                    output_index=0,
                    content_index=0,
                    text=text,
                )
                yield emit("response.output_item.done", output_index=0, item=item)

            for _, tool_call in sorted(tool_calls.items()):
                item = tool_output_item(tool_call)
                output_index = len(output)
                output.append(item)
                yield emit("response.output_item.done", output_index=output_index, item=item)

            completed_response = base_response(response_id, model, "completed", output, usage)
            yield emit("response.completed", response=completed_response)
        except Exception as exc:
            logger.exception("Error translating upstream Responses stream")
            failed_response = base_response(response_id, model, "failed", [])
            failed_response["error"] = {"code": "proxy_stream_error", "message": str(exc)}
            yield emit("response.failed", response=failed_response)
        finally:
            await upstream_response.aclose()
            await client.aclose()

    return StreamingResponse(
        responses_stream_generator(),
        media_type="text/event-stream",
        headers={
            "Cache-Control": "no-cache",
            "Connection": "keep-alive",
            "X-Accel-Buffering": "no",
        },
    )


if __name__ == "__main__":
    import uvicorn

    if not config.DEFAULT_API_KEY and sys.stdin.isatty():
        try:
            key_input = getpass.getpass("Enter your Freemodel API Key: ")
            if key_input.strip():
                config.DEFAULT_API_KEY = key_input.strip()
        except Exception:
            pass
    uvicorn.run("proxy_server:app", host=DEFAULT_HOST, port=DEFAULT_PORT, reload=False)
