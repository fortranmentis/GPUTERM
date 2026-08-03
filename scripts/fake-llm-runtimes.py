"""Fake Ollama and vLLM servers for manual testing.

Serves fixed responses on 127.0.0.1:18434 (Ollama) and 127.0.0.1:18000 (vLLM)
so the real HTTP client can be exercised without a GPU box. The vLLM side
requires `Authorization: Bearer sk-test` on /health, which is how the
authentication path is checked.

    python3 scripts/fake-llm-runtimes.py &
    cargo test --manifest-path src-tauri/Cargo.toml --lib live_ -- --ignored --nocapture

The unit tests need none of this; they use an in-process fake client.
"""

import json, threading
from http.server import BaseHTTPRequestHandler, HTTPServer

PS = {"models":[{"name":"llama3:8b","model":"llama3:8b","size":1000,"size_vram":800,
                 "context_length":8192,"expires_at":"2099-01-01T00:00:00Z",
                 "details":{"parameter_size":"8B","quantization_level":"Q4_0","family":"llama"}}]}
TAGS = {"models":[{"name":"llama3:8b","model":"llama3:8b","size":1000},
                  {"name":"qwen:7b","model":"qwen:7b","size":2000}]}
MODELS = {"object":"list","data":[{"id":"meta-llama/Llama-3-8B","object":"model","owned_by":"vllm"}]}
METRICS = """# HELP vllm:num_requests_running Number of requests currently running.
# TYPE vllm:num_requests_running gauge
vllm:num_requests_running{engine="0",model_name="meta-llama/Llama-3-8B"} 3.0
# TYPE vllm:num_requests_waiting gauge
vllm:num_requests_waiting{engine="0",model_name="meta-llama/Llama-3-8B"} 1.0
# TYPE vllm:kv_cache_usage_perc gauge
vllm:kv_cache_usage_perc{engine="0",model_name="meta-llama/Llama-3-8B"} 0.73
# TYPE vllm:prompt_tokens_total counter
vllm:prompt_tokens_total{model_name="meta-llama/Llama-3-8B"} 1000
# TYPE vllm:time_to_first_token_seconds histogram
vllm:time_to_first_token_seconds_bucket{le="0.1",model_name="m"} 10
vllm:time_to_first_token_seconds_bucket{le="0.5",model_name="m"} 60
vllm:time_to_first_token_seconds_bucket{le="+Inf",model_name="m"} 100
vllm:time_to_first_token_seconds_count{model_name="m"} 100
"""

class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        auth = self.headers.get("Authorization")
        if self.path == "/api/ps": return self.send_json(PS)
        if self.path == "/api/tags": return self.send_json(TAGS)
        if self.path == "/health":
            if auth != "Bearer sk-test":
                return self.send_body(401, "text/plain", "unauthorized")
            return self.send_body(200, "text/plain", "")
        if self.path == "/v1/models": return self.send_json(MODELS)
        if self.path == "/metrics": return self.send_body(200, "text/plain", METRICS)
        if self.path == "/dead": return self.send_body(503, "text/plain", "engine dead")
        self.send_body(404, "text/plain", "no")
    def send_json(self, obj): self.send_body(200, "application/json", json.dumps(obj))
    def send_body(self, code, ctype, body):
        raw = body.encode()
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

for port in (18434, 18000):
    threading.Thread(target=HTTPServer(("127.0.0.1", port), Handler).serve_forever, daemon=True).start()
print("listening on 18434 and 18000", flush=True)
threading.Event().wait()
