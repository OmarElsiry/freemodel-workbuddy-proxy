"""FastAPI proxy server bridging OpenAI-compatible clients to Freemodel API.

Provides 100% robust handling for Codex App, OpenCode, Cursor, Continue, and standard OpenAI clients.
"""

from fastapi import FastAPI, Request, HTTPException
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import StreamingResponse, JSONResponse
import httpx
import json
import time
import uuid
import sys
import getpass
import logging
import config
from config import DEFAULT_BASE_URL, CLIENT_HEADERS, AVAILABLE_MODELS, DEFAULT_HOST, DEFAULT_PORT

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger("freemodel-proxy")

app = FastAPI(title="Freemodel API Proxy", version="1.0.0")

app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

def normalize_model(model_name: str | None) -> str:
    """Normalize model string to canonical gpt-5.6-sol model name."""
    if not model_name:
        return "gpt-5.6-sol"
    cleaned = model_name.strip().lower()
    if cleaned in ["gpt 5.6 sol", "gpt-5.6-sol", "gpt-5.6", "gpt 5.6", "opencode-default", "gpt-4o", "gpt-4"]:
        return "gpt-5.6-sol"
    return model_name

def get_auth_token(request: Request) -> str:
    """Extract Authorization header or fallback to configured API key."""
    auth = request.headers.get("authorization")
    if auth and auth.startswith("Bearer ") and len(auth.split(" ")) > 1:
        token = auth.split(" ")[1].strip()
        if token and token not in ["sk-dummy", "dummy", "placeholder", ""]:
            return f"Bearer {token}"
    key = config.DEFAULT_API_KEY or config.load_saved_key()
    if key:
        return f"Bearer {key}"
    return ""

def extract_last_user_message(messages: list) -> str:
    """Extract text from the last user message in payload."""
    for msg in reversed(messages):
        if isinstance(msg, dict) and msg.get("role") == "user":
            content = msg.get("content", "")
            if isinstance(content, str):
                return content
            elif isinstance(content, list):
                parts = [p.get("text", "") for p in content if isinstance(p, dict) and p.get("type") in ["text", "input_text"]]
                return " ".join(parts) if parts else str(content)
    return "Hello"

@app.get("/")
@app.get("/health")
async def health_check():
    current_base = config.DEFAULT_BASE_URL
    return {"status": "ok", "service": "freemodel-proxy", "upstream": current_base}

@app.get("/v1/models")
async def list_models():
    return {"object": "list", "data": AVAILABLE_MODELS}

@app.post("/v1/chat/completions")
async def chat_completions(request: Request):
    try:
        body = await request.json()
    except Exception as e:
        raise HTTPException(status_code=400, detail=f"Invalid JSON payload: {e}")

    model = normalize_model(body.get("model"))
    body["model"] = model

    stream = body.get("stream", False)
    auth_header = get_auth_token(request)

    base_url = config.DEFAULT_BASE_URL
    upstream_headers = {
        "Content-Type": "application/json",
        **CLIENT_HEADERS
    }
    if auth_header:
        upstream_headers["Authorization"] = auth_header

    target_url = f"{base_url.rstrip('/')}/chat/completions"
    logger.info(f"Proxying chat/completions (model={model}, stream={stream}) -> {target_url}")

    if stream:
        async def stream_generator():
            try:
                async with httpx.AsyncClient(timeout=120.0) as client:
                    async with client.stream("POST", target_url, json=body, headers=upstream_headers) as response:
                        if response.status_code == 200:
                            async for chunk in response.aiter_bytes():
                                yield chunk
                            return
                        else:
                            err_body = await response.aread()
                            err_str = err_body.decode('utf-8', errors='ignore')
                            logger.error(f"Upstream status {response.status_code}: {err_str}")
                            err_msg = f"[Upstream Error {response.status_code}]: {err_str}"
            except Exception as e:
                logger.error(f"Upstream stream connection error: {e}")
                err_msg = f"[Proxy Connection Error]: {e}"

            cmpl_id = f"chatcmpl-{uuid.uuid4().hex[:12]}"
            chunk_content = {
                "id": cmpl_id,
                "object": "chat.completion.chunk",
                "created": int(time.time()),
                "model": model,
                "choices": [{"index": 0, "delta": {"content": err_msg}, "finish_reason": "stop"}]
            }
            yield f"data: {json.dumps(chunk_content)}\n\n".encode("utf-8")
            yield b"data: [DONE]\n\n"

        return StreamingResponse(stream_generator(), media_type="text/event-stream")

    else:
        try:
            async with httpx.AsyncClient(timeout=120.0) as client:
                resp = await client.post(target_url, json=body, headers=upstream_headers)
                try:
                    res_json = resp.json()
                    return JSONResponse(status_code=resp.status_code, content=res_json)
                except Exception:
                    return JSONResponse(
                        status_code=resp.status_code,
                        content={"error": {"message": resp.text, "code": resp.status_code}}
                    )
        except Exception as e:
            logger.error(f"Upstream request exception: {e}")
            return JSONResponse(
                status_code=502,
                content={"error": {"message": f"Bad Gateway: {e}", "type": "proxy_error"}}
            )


@app.post("/v1/responses")
async def responses_endpoint(request: Request):
    """Adapter for OpenCode / Codex Responses API format."""
    try:
        body = await request.json()
    except Exception as e:
        raise HTTPException(status_code=400, detail=f"Invalid JSON payload: {e}")

    messages = []
    if "input" in body and isinstance(body["input"], list):
        for msg in body["input"]:
            role = msg.get("role", "user")
            content = msg.get("content", "")
            if isinstance(content, list):
                text_parts = [part.get("text", "") for part in content if isinstance(part, dict) and part.get("type") == "input_text"]
                content = "".join(text_parts) if text_parts else str(content)
            messages.append({"role": role, "content": content})
    elif "messages" in body:
        messages = body["messages"]
    else:
        messages = [{"role": "user", "content": str(body.get("prompt", ""))}]

    model = normalize_model(body.get("model"))
    chat_payload = {
        "model": model,
        "messages": messages,
        "stream": body.get("stream", False)
    }

    auth_header = get_auth_token(request)
    base_url = config.DEFAULT_BASE_URL
    upstream_headers = {
        "Content-Type": "application/json",
        **CLIENT_HEADERS
    }
    if auth_header:
        upstream_headers["Authorization"] = auth_header

    target_url = f"{base_url.rstrip('/')}/chat/completions"

    if chat_payload["stream"]:
        async def stream_generator():
            try:
                async with httpx.AsyncClient(timeout=120.0) as client:
                    async with client.stream("POST", target_url, json=chat_payload, headers=upstream_headers) as response:
                        if response.status_code == 200:
                            async for chunk in response.aiter_bytes():
                                yield chunk
                            return
                        else:
                            err_body = await response.aread()
                            err_str = err_body.decode('utf-8', errors='ignore')
                            err_msg = f"[Upstream Error {response.status_code}]: {err_str}"
            except Exception as e:
                logger.error(f"Upstream responses stream exception: {e}")
                err_msg = f"[Proxy Connection Error]: {e}"

            cmpl_id = f"chatcmpl-{uuid.uuid4().hex[:12]}"
            chunk_content = {
                "id": cmpl_id,
                "object": "chat.completion.chunk",
                "created": int(time.time()),
                "model": model,
                "choices": [{"index": 0, "delta": {"content": err_msg}, "finish_reason": "stop"}]
            }
            yield f"data: {json.dumps(chunk_content)}\n\n".encode("utf-8")
            yield b"data: [DONE]\n\n"

        return StreamingResponse(stream_generator(), media_type="text/event-stream")

    try:
        async with httpx.AsyncClient(timeout=120.0) as client:
            resp = await client.post(target_url, json=chat_payload, headers=upstream_headers)
            try:
                return JSONResponse(status_code=resp.status_code, content=resp.json())
            except Exception:
                return JSONResponse(
                    status_code=resp.status_code,
                    content={"error": {"message": resp.text, "code": resp.status_code}}
                )
    except Exception as e:
        logger.error(f"Upstream responses request exception: {e}")
        return JSONResponse(
            status_code=502,
            content={"error": {"message": f"Bad Gateway: {e}", "type": "proxy_error"}}
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
