"""Exhaustive Edge-Case & Integration Test Suite for Freemodel API Proxy."""

import httpx
import json
import sys

BASE_URL = "http://127.0.0.1:40589"
TEST_KEY = "fe_oa_64748ef59ad0c85cb0da2f6a9f1ebc0fb3b2ca94fc05feb3"

def run_exhaustive_tests():
    print("==========================================================")
    print("      EXHAUSTIVE TEST SUITE - FREEMODEL PROXY            ")
    print("==========================================================")
    
    passed = 0
    failed = 0

    headers = {"Authorization": f"Bearer {TEST_KEY}"}

    with httpx.Client(timeout=20.0) as client:
        # Test 1: GET /health
        try:
            r = client.get(f"{BASE_URL}/health")
            assert r.status_code == 200
            assert r.json()["status"] == "ok"
            print("[PASS 1/15] GET /health -> Status 200 OK")
            passed += 1
        except Exception as e:
            print("[FAIL 1/15] GET /health:", e)
            failed += 1

        # Test 2: GET /v1/models
        try:
            r = client.get(f"{BASE_URL}/v1/models")
            assert r.status_code == 200
            data = r.json().get("data", [])
            assert len(data) >= 4
            print(f"[PASS 2/15] GET /v1/models -> {len(data)} models listed")
            passed += 1
        except Exception as e:
            print("[FAIL 2/15] GET /v1/models:", e)
            failed += 1

        # Test 3: POST /v1/chat/completions (model="gpt-5.6-sol")
        try:
            p = {"model": "gpt-5.6-sol", "messages": [{"role": "user", "content": "Hello"}]}
            r = client.post(f"{BASE_URL}/v1/chat/completions", json=p, headers=headers)
            assert r.status_code == 200
            assert "choices" in r.json()
            print("[PASS 3/15] POST /v1/chat/completions (gpt-5.6-sol)")
            passed += 1
        except Exception as e:
            print("[FAIL 3/15] POST /v1/chat/completions (gpt-5.6-sol):", e)
            failed += 1

        # Test 4: POST /v1/chat/completions (model="gpt 5.6 sol" with space)
        try:
            p = {"model": "gpt 5.6 sol", "messages": [{"role": "user", "content": "Test space model"}]}
            r = client.post(f"{BASE_URL}/v1/chat/completions", json=p, headers=headers)
            assert r.status_code == 200
            assert "choices" in r.json()
            print("[PASS 4/15] POST /v1/chat/completions (model name normalization)")
            passed += 1
        except Exception as e:
            print("[FAIL 4/15] POST /v1/chat/completions (model name normalization):", e)
            failed += 1

        # Test 5: POST /v1/chat/completions (model="gpt-4o")
        try:
            p = {"model": "gpt-4o", "messages": [{"role": "user", "content": "Test alias"}]}
            r = client.post(f"{BASE_URL}/v1/chat/completions", json=p, headers=headers)
            assert r.status_code == 200
            print("[PASS 5/15] POST /v1/chat/completions (gpt-4o alias)")
            passed += 1
        except Exception as e:
            print("[FAIL 5/15] POST /v1/chat/completions (gpt-4o alias):", e)
            failed += 1

        # Test 6: POST /v1/chat/completions (model="opencode-default")
        try:
            p = {"model": "opencode-default", "messages": [{"role": "user", "content": "OpenCode test"}]}
            r = client.post(f"{BASE_URL}/v1/chat/completions", json=p, headers=headers)
            assert r.status_code == 200
            print("[PASS 6/15] POST /v1/chat/completions (opencode-default alias)")
            passed += 1
        except Exception as e:
            print("[FAIL 6/15] POST /v1/chat/completions (opencode-default alias):", e)
            failed += 1

        # Test 7: POST /v1/chat/completions (unknown custom model)
        try:
            p = {"model": "custom-llm-v1", "messages": [{"role": "user", "content": "Custom model test"}]}
            r = client.post(f"{BASE_URL}/v1/chat/completions", json=p, headers=headers)
            assert r.status_code == 200
            print("[PASS 7/15] POST /v1/chat/completions (custom model fallback handling)")
            passed += 1
        except Exception as e:
            print("[FAIL 7/15] POST /v1/chat/completions (custom model):", e)
            failed += 1

        # Test 8: POST /v1/chat/completions (streaming)
        try:
            p = {"model": "gpt-5.6-sol", "messages": [{"role": "user", "content": "Stream test"}], "stream": True}
            with client.stream("POST", f"{BASE_URL}/v1/chat/completions", json=p, headers=headers) as resp:
                assert resp.status_code == 200
                lines = [l for l in resp.iter_lines() if l]
                assert len(lines) > 0
                print(f"[PASS 8/15] POST /v1/chat/completions (SSE streaming, {len(lines)} lines)")
                passed += 1
        except Exception as e:
            print("[FAIL 8/15] POST /v1/chat/completions (streaming):", e)
            failed += 1

        # Test 9: Multimodal payload structure
        try:
            p = {
                "model": "gpt-5.6-sol",
                "messages": [
                    {
                        "role": "user",
                        "content": [
                            {"type": "text", "text": "Multimodal text block"}
                        ]
                    }
                ]
            }
            r = client.post(f"{BASE_URL}/v1/chat/completions", json=p, headers=headers)
            assert r.status_code == 200
            print("[PASS 9/15] POST /v1/chat/completions (multimodal message body format)")
            passed += 1
        except Exception as e:
            print("[FAIL 9/15] POST /v1/chat/completions (multimodal):", e)
            failed += 1

        # Test 10: Multi-turn history (System + User + Assistant + User)
        try:
            p = {
                "model": "gpt-5.6-sol",
                "messages": [
                    {"role": "system", "content": "You are a helpful coding assistant."},
                    {"role": "user", "content": "What is 2+2?"},
                    {"role": "assistant", "content": "2+2 is 4."},
                    {"role": "user", "content": "Multiply that by 10."}
                ]
            }
            r = client.post(f"{BASE_URL}/v1/chat/completions", json=p, headers=headers)
            assert r.status_code == 200
            print("[PASS 10/15] POST /v1/chat/completions (multi-turn conversation history)")
            passed += 1
        except Exception as e:
            print("[FAIL 10/15] POST /v1/chat/completions (multi-turn):", e)
            failed += 1

        # Test 11: OpenCode Responses API format (non-streaming)
        try:
            p = {
                "model": "gpt-5.6-sol",
                "input": [
                    {
                        "type": "message",
                        "role": "user",
                        "content": [{"type": "input_text", "text": "OpenCode Responses test"}]
                    }
                ]
            }
            r = client.post(f"{BASE_URL}/v1/responses", json=p, headers=headers)
            assert r.status_code == 200
            print("[PASS 11/15] POST /v1/responses (OpenCode non-streaming format)")
            passed += 1
        except Exception as e:
            print("[FAIL 11/15] POST /v1/responses (non-streaming):", e)
            failed += 1

        # Test 12: OpenCode Responses API format (streaming)
        try:
            p = {
                "model": "gpt-5.6-sol",
                "input": [
                    {
                        "type": "message",
                        "role": "user",
                        "content": [{"type": "input_text", "text": "OpenCode Responses stream"}]
                    }
                ],
                "stream": True
            }
            with client.stream("POST", f"{BASE_URL}/v1/responses", json=p, headers=headers) as resp:
                assert resp.status_code == 200
                lines = [l for l in resp.iter_lines() if l]
                assert len(lines) > 0
                print(f"[PASS 12/15] POST /v1/responses (OpenCode SSE streaming, {len(lines)} lines)")
                passed += 1
        except Exception as e:
            print("[FAIL 12/15] POST /v1/responses (streaming):", e)
            failed += 1

        # Test 13: Request without Authorization header
        try:
            p = {"model": "gpt-5.6-sol", "messages": [{"role": "user", "content": "No auth header"}]}
            r = client.post(f"{BASE_URL}/v1/chat/completions", json=p)
            assert r.status_code == 200
            print("[PASS 13/15] POST /v1/chat/completions (missing Auth header gracefully handled)")
            passed += 1
        except Exception as e:
            print("[FAIL 13/15] POST /v1/chat/completions (missing Auth):", e)
            failed += 1

        # Test 14: Request with sk-dummy header
        try:
            p = {"model": "gpt-5.6-sol", "messages": [{"role": "user", "content": "Dummy token"}]}
            r = client.post(f"{BASE_URL}/v1/chat/completions", json=p, headers={"Authorization": "Bearer sk-dummy"})
            assert r.status_code == 200
            print("[PASS 14/15] POST /v1/chat/completions (placeholder sk-dummy token handled)")
            passed += 1
        except Exception as e:
            print("[FAIL 14/15] POST /v1/chat/completions (sk-dummy token):", e)
            failed += 1

        # Test 15: CORS OPTIONS Preflight
        try:
            r = client.options(
                f"{BASE_URL}/v1/chat/completions",
                headers={
                    "Origin": "http://localhost:3000",
                    "Access-Control-Request-Method": "POST",
                    "Access-Control-Request-Headers": "content-type,authorization"
                }
            )
            assert r.status_code == 200
            assert "access-control-allow-origin" in r.headers
            print("[PASS 15/15] OPTIONS /v1/chat/completions (CORS Preflight headers returned)")
            passed += 1
        except Exception as e:
            print("[FAIL 15/15] OPTIONS /v1/chat/completions (CORS):", e)
            failed += 1

    print("\n==========================================================")
    print(f"       RESULT: {passed} PASSED, {failed} FAILED               ")
    print("==========================================================")
    return 0 if failed == 0 else 1

if __name__ == "__main__":
    sys.exit(run_exhaustive_tests())
