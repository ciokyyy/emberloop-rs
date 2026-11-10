# Live FigCraft session driver — FALLBACK ONLY

Use this only on a host that has no `figcraft.*` MCP tools in `execute`. With those tools available, call them directly (`tools.call("figcraft.batch", …)`) and never write a script: `figcraft-cli mcp` speaks newline-delimited JSON-RPC, and joining the project's live document is what makes edits land; a plain `figcraft-cli run` script refuses the join.

```python
import subprocess, json, time, os

root = "/root/projects/emberloop-rs"
env = dict(os.environ,
           FIGCRAFT_LIVE_DIR="/root/.config/emberloop-rs/figcraft/projects",
           FIGCRAFT_SERVER_DATA="/root/.config/emberloop-rs/figcraft/server")
link = json.load(open(env["FIGCRAFT_LIVE_DIR"] + "/<sha256-of-canonical-root>.json"))["link"]
p = subprocess.Popen(["figcraft-cli", "mcp"], cwd=root, env=env, stdin=subprocess.PIPE,
                     stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, bufsize=1)
_id = [0]

def send(msg):
    p.stdin.write(json.dumps(msg) + "\n"); p.stdin.flush()

def recv():
    line = p.stdout.readline()
    return json.loads(line) if line.strip() else None

def call(tool, args):
    _id[0] += 1
    send({"jsonrpc": "2.0", "id": _id[0], "method": "tools/call",
          "params": {"name": tool, "arguments": args}})
    while True:
        msg = recv()
        if msg and msg.get("id") == _id[0]:
            if "error" in msg:
                raise RuntimeError(msg["error"])
            if msg["result"].get("isError"):
                raise RuntimeError(msg["result"]["content"][0]["text"])
            text = msg["result"]["content"][0]["text"] if msg["result"].get("content") else ""
            return json.loads(text) if text.strip().startswith(("{", "[")) else text

def cmd(tool, **params):
    return call("run_command", {"command": tool, "params": params})

send({"jsonrpc": "2.0", "id": 1, "method": "initialize",
      "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                 "clientInfo": {"name": "design", "version": "0"}}})
recv()
send({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}})
cmd("collab.join", link=link)

# Readiness is a read, not the peers status: an unattached client answers null here
# and would save a local copy that the daemon snapshot then reverts. The cheapest
# check is one inspect_document: it must name the project and list page children
# (an empty local copy answers name "Untitled" with no children). The peers status
# flips from "connecting" to "live" about a second after joining; poll the read.
def attached(node_id):
    got = cmd("node.get", id=node_id) or {}
    return got.get("id") == node_id

known_id = <any id from the page's children, or one you created this session>
for _ in range(40):
    if attached(known_id):
        break
    time.sleep(0.25)
else:
    raise SystemExit("join did not attach; re-run collab.join")

# an imported svg lands at page coordinates, then nests without moving
icon = cmd("file.import", path=f"{root}/.tmp/icons/mic.svg", x=678, y=120, name="Mic")["id"]
cmd("node.reparent", ids=[icon], parent=frame_id)
call("save_file", {"path": f"{root}/design.figcraft"})
p.stdin.close(); p.wait(timeout=15)
```

MCP tool names on this server: `list_commands`, `run_command`, `batch`, `create_node`, `set_properties`, `inspect_document`, `get_node`, `select`, `export`, `screenshot`, `open_file`, `save_file`, `undo`, `redo`, `pointer`, `key`, `type_text`, `inspect_ui`, `wait_for_changes`.

`readline` must be matched on `id`, never read positionally: a `notifications/initialized` is answered with `method not found: notifications/initialized`, and a driver that reads one line per request silently shifts every later answer by one (the peers payload arrives where a `node.get` was expected, and the frame looks childless).

## Restack an existing mock

Move layers you already drew rather than deleting and redrawing them, so ids, text styles and icon frames survive:

```python
for node_id, props in [
    (title_id,   {"x": 40, "y": 30}),
    (field_id,   {"x": 12, "y": 56, "width": 336, "height": 56, "cornerRadius": 16}),
    (placeholder,{"x": 24, "y": 70, "characters": "Message the agent…"}),
    (frame_id,   {"height": 170}),
]:
    cmd("node.set", id=node_id, **props)
call("save_file", {"path": f"{root}/design.figcraft"})
```

Read the frame back with `node.get` after the writes: `x`/`y` can report an offset value once, so re-set anything that reads wrong and confirm the children's final geometry before saving.

## Prove the save survived

`save_file` returning a byte count only proves the write that moment. Sleep past the snapshot interval, stat the file again, and re-read the frame's children in a fresh session; if the size went back up or the old children are there, the daemon overwrote the edit, which means the earlier writes went to a local copy.

## Import a package svg at a rendered size

A lucide svelte file carries its geometry as data, so no path is retyped:

```python
import re, json
src = open("web/node_modules/@lucide/svelte/dist/icons/mic.svelte").read()
data = json.loads(re.search(r"__iconData = (\{.*?\});\n", src, re.S).group(1))
els = "".join("<" + tag + " " + " ".join(f'{k}="{v}"' for k, v in attrs.items()) + "/>"
              for tag, attrs in data["node"])
svg = (f'<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" '
       f'fill="none" stroke="#c7c7cc" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">{els}</svg>')
```

`Icon.svelte` draws a 24-box at `stroke-width="2"`, so a 16px icon renders at an effective 1.33 stroke; an 18px `ContextRing` scales its 20-box the same way. Brand logos are already files under `@lobehub/icons-static-svg/icons/`: rewrite `width`/`height` and replace `currentColor` with the tint, keeping `viewBox`.

## Verify the render

```python
import zlib, struct
data = open("/tmp/x.png", "rb").read()
pos, idat, w, h, ct = 8, b"", 0, 0, 0
while pos < len(data):
    ln = struct.unpack(">I", data[pos:pos + 4])[0]; typ = data[pos + 4:pos + 8]
    chunk = data[pos + 8:pos + 8 + ln]; pos += 12 + ln
    if typ == b"IHDR": w, h, _, ct = struct.unpack(">IIBB", chunk[:10])
    elif typ == b"IDAT": idat += chunk
raw = zlib.decompress(idat); bpp = 4 if ct == 6 else 3; stride = w * bpp
rows, prev, i = [], bytearray(stride), 0
for _ in range(h):
    f = raw[i]; i += 1; line = bytearray(raw[i:i + stride]); i += stride
    for x in range(stride):
        a = line[x - bpp] if x >= bpp else 0; b = prev[x]
        c = prev[x - bpp] if x >= bpp else 0
        if f == 1: line[x] = (line[x] + a) & 255
        elif f == 2: line[x] = (line[x] + b) & 255
        elif f == 3: line[x] = (line[x] + (a + b) // 2) & 255
        elif f == 4:
            p = a + b - c; pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
            line[x] = (line[x] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
    rows.append(line); prev = line

def px(x, y, scale=2):
    row = rows[int(y * scale)]
    return tuple(row[int(x * scale) * bpp:int(x * scale) * bpp + 3])
```

Compare the sampled fill, border and icon ink against the app tokens in `web/src/app.css`, and count non-surface pixels inside an icon's `absoluteBounds` to prove the glyph drew rather than a placeholder rectangle.

## Export the frame you are checking

Export one frame at a time and the image origin is the frame's own top-left, so `pixel = local * scale` with no offset arithmetic:

```python
call("figcraft.export", {"ids": [frame_id], "format": "png", "scale": 2, "path": "/tmp/f.png"})
# 1120 x 992 for a 560 x 496 frame at scale 2
```

Exporting several frames in one call pads the canvas, offsets each frame and can return them in a different order, so a shared origin is wrong. Read the IHDR size back and divide by the frame size to confirm the mapping before sampling. Verify 1 px hairlines at `scale: 1`: at scale 2 a hairline can drop out of the render while the layer is still correct.

## Check text with an ink box

A text node's `x`/`width` are the box, so they hide clipping and overlap. Measure the ink instead: scan the box (plus a small pad) for pixels that differ from the local background and compare the extremes with the intended edge.

```python
def ink(px, x, y, w, h, bg, pad=4, scale=2):
    mnx = mny = 10**9; mxx = mxy = -1
    for yy in range(int((y-pad)*scale), int((y+h+pad)*scale)):
        for xx in range(int((x-pad)*scale), int((x+w+pad)*scale)):
            c = px(xx, yy)
            if sum(abs(c[i]-bg[i]) for i in range(3)) > 60:
                mnx, mny = min(mnx, xx), min(mny, yy)
                mxx, mxy = max(mxx, xx), max(mxy, yy)
    return None if mxx < 0 else (mnx/scale, mny/scale, mxx/scale, mxy/scale)
```

Right-aligned columns must share one ink right edge; a value and its suffix must not share pixels; a one-line description must stay inside its width. Backgrounds differ per region (frame fill, card fill, selected row, button), so pass the colour of the layer the text sits on.
