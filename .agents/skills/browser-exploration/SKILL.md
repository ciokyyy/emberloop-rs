---
name: browser-exploration
description: Use when driving or debugging a page in a browser from a Session (screenshots, console errors, network, hydration timing) or when choosing between agent-browser, playwright-cli and a browser MCP server.
---

# Browser exploration

## Pick the tool

`execute` is the only route to any browser tool. An MCP server costs nothing
until it is called: `mcpclient.rs` loads `<config root>/mcp.json` and exposes
each tool as `tools.<server>.<tool>(...)`, so its schemas never enter the
model's tool list. Cost is the tool's *output*, plus one `tools.search`/`help`
call to learn its names.

| Tool | Tools | Catalog tokens | Runtime footprint | Debugging | Verdict |
|---|---|---|---|---|---|
| `agent-browser` (installed) | ~90, one binary | 0 (CLI help) | Rust + Chrome | console, errors, network, trace, profiler, react tree, vitals, a11y, diff | default |
| `chrome-devtools-mcp --slim` | 3 | ~359 (claim, unverified) | Node + Chrome | navigate, evaluate, screenshot | best small MCP |
| `charlotte` | 7 / 23 / 43 profiles | profiled, published benchmarks | Node + Chrome | console, requests, dev_serve | good profiles, young |
| `caveman-browse` | 4 | 297 (measured) | one Go binary + Chrome | weak | token winner, thin |
| `microsoft/playwright-mcp` | 24 | 3.4k (measured) | Node + Chromium | weak for debugging | avoid, upstream says use its CLI |
| `hangwin/mcp-chrome` | 20+ | large | no bundled browser | network, screenshots | uses your real Chrome |
| `browserbase` MCP | 6 | small | cloud | logs only | zero local RAM, network + cost |

Measured here: 17 Chromium processes, 1432 MB RSS total for one page. The
browser is the RAM cost; the wrapper is noise. Nothing beats Chrome for
debugging, and nothing beats "few tools, compact text" for tokens. Both
requirements point at the same shape: one small tool set over CDP.

## Chrome is already resolved

`browser.rs` picks one Chrome for every shell the Session runs and exports
`AGENT_BROWSER_EXECUTABLE_PATH`, a heap-capped `AGENT_BROWSER_ARGS`, and a
5-minute `AGENT_BROWSER_IDLE_TIMEOUT_MS` that closes a forgotten browser. It
searches Playwright's cache, every `pi-homes/*/.agent-browser` cache, and the
system Chrome, newest version first. Just run `agent-browser`; do not export
the variables by hand and do not run `agent-browser install` unless the tool
answers that no Chrome was found.

## The loop

1. `agent-browser open <url>` — prints title and URL only.
2. `agent-browser errors` and `agent-browser console`. A dev server that is
   still mounting shows up here, not in a screenshot.
3. `agent-browser wait <selector>` for the element that proves hydration
   finished. Sleeping is guessing.
4. `agent-browser snapshot` for the accessibility tree with `@eN` refs, then
   act on refs. Text, not pixels.
5. `agent-browser screenshot /tmp/x.png` last, and only as proof for the user.
   Check the file size; a 0-byte or tiny PNG means a blank page.
6. `agent-browser close` when done, so the Session does not leak a browser.

Prefer one `batch` of commands over many calls. Prefer `get text <sel>` and
`eval <js>` over dumping a snapshot when the answer is one value.

## Cap the browser before it eats the machine

A page with a reactive loop (Svelte `$effect` reading state it writes, Vue
watcher ping-pong, React `setState` in render) grows the V8 heap until the box
swaps. The heap cap is already exported: `--max-old-space-size=512` per
renderer. Add a cgroup around the whole browser tree when the machine must
survive: children inherit it, and the kernel kills at the limit instead of at
6 GB.

```bash
C=/sys/fs/cgroup/elchrome
mkdir -p "$C"
printf '1G' > "$C/memory.max"
printf '0' > "$C/memory.swap.max"
sh -c "echo \$\$ > $C/cgroup.procs; exec agent-browser open <url>"
```

Read `"$C/memory.peak"` after the run to see what it really used (example.com
peaks at 284 MB). `memory.events` reports `oom_kill 1` when the cap fired.

Always end with `agent-browser close --all`. `execute` timing out kills
python, not the daemon: leaked browsers are how one machine reached 6 GB. If a
cgroup is wedged, `printf 1 > "$C/cgroup.kill"` clears it.

Detect the loop before the cap fires: `agent-browser console` repeating the
same message, or `eval 'performance.memory.usedJSHeapSize'` climbing between
two calls. Page heap over ~300 MB on a dev page is the loop, not the app.
