import sys, json, time
def send(o): sys.stdout.write(json.dumps(o) + "\n"); sys.stdout.flush()
for line in sys.stdin:
    try: m = json.loads(line)
    except Exception: continue
    method, id_ = m.get("method"), m.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": id_, "result": {"protocolVersion": "2025-03-26", "capabilities": {"tools": {}}, "serverInfo": {"name": "mock", "version": "1"}}})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": id_, "result": {"tools": [
            {"name": "echo", "description": "echo text", "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}, "annotations": {"readOnlyHint": True}},
            {"name": "write", "description": "mutating", "inputSchema": {"type": "object", "properties": {}}},
            {"name": "boom", "description": "fails", "inputSchema": {"type": "object", "properties": {}}},
            {"name": "slow", "description": "sleeps", "inputSchema": {"type": "object", "properties": {}}}]}})
    elif method == "tools/call":
        n, a = m["params"]["name"], m["params"].get("arguments", {})
        if n == "echo": send({"jsonrpc": "2.0", "id": id_, "result": {"content": [{"type": "text", "text": "echo:" + a.get("text", "")}]}})
        elif n == "boom": send({"jsonrpc": "2.0", "id": id_, "result": {"isError": True, "content": [{"type": "text", "text": "it broke"}]}})
        elif n == "slow": time.sleep(30)
        else: send({"jsonrpc": "2.0", "id": id_, "error": {"code": -32601, "message": "unknown tool"}})
    elif id_ is not None:
        send({"jsonrpc": "2.0", "id": id_, "result": {}})
