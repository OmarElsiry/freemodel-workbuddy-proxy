"""Comprehensive test suite for Freemodel API Proxy."""

import httpx
import json
import sys
import os

BASE_URL = "http://127.0.0.1:40589"
TEST_API_KEY = "sk-dummy"

def run_tests():
    print("=== Starting Freemodel Proxy Tests ===")
    passed = 0
    failed = 0

    headers = {
        "Authorization": f"Bearer {TEST_API_KEY}"
    }

    with httpx.Client(timeout=15.0) as client:
        # 1. Health check
        try:
            r = client.get(f"{BASE_URL}/health")
            assert r.status_code == 200, f"Expected 200, got {r.status_code}"
            print("[PASS] GET /health:", r.json())
            passed += 1
        except Exception as e:
            print("[FAIL] GET /health:", e)
            failed += 1

        # 2. List models
        try:
            r = client.get(f"{BASE_URL}/v1/models")
            assert r.status_code == 200
            models = r.json().get("data", [])
            model_ids = [m["id"] for m in models]
            assert "gpt-5.6-sol" in model_ids or "gpt 5.6 sol" in model_ids
            print(f"[PASS] GET /v1/models: {len(models)} models available -> {model_ids}")
            passed += 1
        except Exception as e:
            print("[FAIL] GET /v1/models:", e)
            failed += 1

        # 3. Chat Completions (non-streaming, model="gpt 5.6 sol")
        try:
            payload = {
                "model": "gpt 5.6 sol",
                "messages": [{"role": "user", "content": "Hello AI"}]
            }
            r = client.post(f"{BASE_URL}/v1/chat/completions", json=payload, headers=headers)
            assert r.status_code == 200, f"Status {r.status_code}: {r.text}"
            res_data = r.json()
            content = res_data["choices"][0]["message"]["content"]
            assert content, "No content returned"
            print(f"[PASS] POST /v1/chat/completions (non-streaming): '{content}'")
            passed += 1
        except Exception as e:
            print("[FAIL] POST /v1/chat/completions (non-streaming):", e)
            failed += 1

        # 4. Chat Completions (streaming)
        try:
            payload = {
                "model": "gpt-5.6-sol",
                "messages": [{"role": "user", "content": "Say hi in 3 words"}],
                "stream": True
            }
            with client.stream("POST", f"{BASE_URL}/v1/chat/completions", json=payload, headers=headers) as response:
                assert response.status_code == 200
                chunks = []
                for line in response.iter_lines():
                    if line:
                        chunks.append(line)
                assert len(chunks) > 0
                print(f"[PASS] POST /v1/chat/completions (streaming): received {len(chunks)} SSE lines")
                passed += 1
        except Exception as e:
            print("[FAIL] POST /v1/chat/completions (streaming):", e)
            failed += 1

        # 5. Responses API test (OpenCode format)
        try:
            payload = {
                "model": "gpt-5.6-sol",
                "input": [
                    {
                        "type": "message",
                        "role": "user",
                        "content": [{"type": "input_text", "text": "Testing responses API"}]
                    }
                ]
            }
            r = client.post(f"{BASE_URL}/v1/responses", json=payload, headers=headers)
            assert r.status_code == 200, f"Status {r.status_code}: {r.text}"
            res_data = r.json()
            assert res_data["object"] == "response"
            assert res_data["status"] == "completed"
            assert res_data["output"][0]["type"] == "message"
            content = res_data["output"][0]["content"][0]["text"]
            assert content
            print(f"[PASS] POST /v1/responses: {content}")
            passed += 1
        except Exception as e:
            print("[FAIL] POST /v1/responses:", e)
            failed += 1

    print(f"\nSummary: {passed} passed, {failed} failed.")
    return 0 if failed == 0 else 1

if __name__ == "__main__":
    sys.exit(run_tests())
