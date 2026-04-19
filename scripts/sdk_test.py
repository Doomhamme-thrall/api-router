import json
import os
import traceback

from openai import OpenAI

with open("config/router.json", "r", encoding="utf-8") as f:
    cfg = json.load(f)

api_key = cfg["client_api_keys"][0]
router_base = os.getenv("ROUTER_TEST_BASE_URL", "http://127.0.0.1:8080/v1/")
client = OpenAI(api_key=api_key, base_url=router_base, timeout=30.0)

print("sdk_call_start")
try:
    resp = client.chat.completions.create(
        model="test",
        messages=[{"role": "user", "content": "which llm model you are?"}],
        stream=False,
    )
    content = resp.choices[0].message.content if resp.choices else ""
    content = content or ""
    print("sdk_call_ok", bool(resp.choices), content[:120])
except Exception as exc:
    print("sdk_call_err", type(exc).__name__, str(exc))
    traceback.print_exc()
