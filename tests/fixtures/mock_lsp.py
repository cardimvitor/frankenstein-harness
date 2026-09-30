import sys, json
PULL = "--pull" in sys.argv
docs = {}
def read():
    n = 0
    while True:
        l = sys.stdin.buffer.readline()
        if not l: return None
        l = l.decode().strip()
        if l == "": break
        if l.lower().startswith("content-length:"): n = int(l.split(":")[1])
    return json.loads(sys.stdin.buffer.read(n))
def send(o):
    b = json.dumps(o).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(b) + b); sys.stdout.buffer.flush()
def diags(text):
    out = []
    for i, line in enumerate(text.split("\n")):
        if "ERROR" in line:
            out.append({"range": {"start": {"line": i, "character": 0}, "end": {"line": i, "character": 5}}, "severity": 1, "message": "found " + line.strip()})
        if "WARN" in line:
            out.append({"range": {"start": {"line": i, "character": 0}, "end": {"line": i, "character": 4}}, "severity": 2, "message": "warning " + line.strip()})
    return out
while True:
    m = read()
    if m is None: break
    method = m.get("method")
    if method == "initialize":
        caps = {"textDocumentSync": 1}
        if PULL: caps["diagnosticProvider"] = {"interFileDependencies": False, "workspaceDiagnostics": False}
        # a server-initiated request before answering must not deadlock the client
        send({"jsonrpc": "2.0", "id": 900, "method": "workspace/configuration", "params": {"items": [{}]}})
        send({"jsonrpc": "2.0", "id": m["id"], "result": {"capabilities": caps}})
    elif method in ("textDocument/didOpen", "textDocument/didChange"):
        p = m["params"]["textDocument"]
        text = p["text"] if method == "textDocument/didOpen" else m["params"]["contentChanges"][0]["text"]
        docs[p["uri"]] = text
        if not PULL:
            send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {"uri": p["uri"], "diagnostics": diags(text)}})
    elif method == "textDocument/diagnostic":
        send({"jsonrpc": "2.0", "id": m["id"], "result": {"kind": "full", "items": diags(docs.get(m["params"]["textDocument"]["uri"], ""))}})
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": m["id"], "result": None})
    elif method == "exit":
        break
