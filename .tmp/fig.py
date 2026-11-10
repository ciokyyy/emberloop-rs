"""Drive the live FigCraft session. Usage: python3 .tmp/fig.py .tmp/cmds.json"""
import json, os, subprocess, sys, time

root = "/root/projects/emberloop-rs"
env = dict(os.environ,
           FIGCRAFT_LIVE_DIR="/root/.config/emberloop-rs/figcraft/projects",
           FIGCRAFT_SERVER_DATA="/root/.config/emberloop-rs/figcraft/server")
link = json.load(open(env["FIGCRAFT_LIVE_DIR"] +
                      "/db0200ecd78ef370a30cbf5b3b5547ee990f733a3a4926e80194b91c8e108907.json"))["link"]
p = subprocess.Popen(["figcraft-cli", "mcp"], cwd=root, env=env, stdin=subprocess.PIPE,
                     stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, bufsize=1)
_id = [0]

def recv():
    line = p.stdout.readline()
    return json.loads(line) if line.strip() else None

def call(tool, args):
    _id[0] += 1
    p.stdin.write(json.dumps({"jsonrpc": "2.0", "id": _id[0], "method": "tools/call",
                              "params": {"name": tool, "arguments": args}}) + "\n")
    p.stdin.flush()
    while True:
        msg = recv()
        if msg and msg.get("id") == _id[0]:
            if "error" in msg:
                raise RuntimeError(msg["error"])
            res = msg["result"]
            if res.get("isError"):
                raise RuntimeError(res["content"][0]["text"])
            content = res.get("content") or []
            text = content[0]["text"] if content else ""
            if text.strip().startswith(("{", "[")):
                return json.loads(text)
            return {"_raw": text, "_types": [c.get("type") for c in content]}

def cmd(command, **params):
    return call("run_command", {"command": command, "params": params})

p.stdin.write(json.dumps({"jsonrpc": "2.0", "id": 0, "method": "initialize",
                          "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                                     "clientInfo": {"name": "design", "version": "0"}}}) + "\n")
recv()
p.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}}) + "\n")
p.stdin.flush()
cmd("collab.join", link=link)
time.sleep(1.5)

def tree(node, depth=0, out=None):
    out = [] if out is None else out
    b = node.get("absoluteBounds") or {}
    fill = (node.get("fills") or [{}])[0].get("color") if node.get("fills") else None
    stroke = (node.get("strokes") or [{}])[0].get("color") if node.get("strokes") else None
    bits = [f"{node.get('type')} '{node.get('name')}' @({b.get('x')},{b.get('y')}) {b.get('width')}x{b.get('height')}"]
    if node.get("characters"): bits.append(repr(node["characters"][:48]))
    if node.get("fontSize"): bits.append(f"{node.get('fontFamily')} {node['fontSize']}/{node.get('fontStyle')}")
    if fill: bits.append(f"fill={fill}")
    if stroke: bits.append(f"stroke={stroke}")
    out.append("  " * depth + " ".join(bits))
    for child in node.get("children") or []:
        tree(child, depth + 1, out)
    return out

for item in json.load(open(sys.argv[1])):
    name, args = item[0], item[1] if len(item) > 1 else {}
    try:
        if name == "tree":
            print("\n".join(tree(cmd("node.get", **args))))
            continue
        result = call(name[4:], args) if name.startswith("mcp:") else cmd(name, **args)
        print(json.dumps({"cmd": name, "result": result}))
    except Exception as exc:  # noqa: BLE001 - report and keep going
        print(json.dumps({"cmd": name, "error": str(exc)}))

p.stdin.close()
try:
    p.wait(timeout=15)
except subprocess.TimeoutExpired:
    p.kill()
