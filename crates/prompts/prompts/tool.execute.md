Run a Python script (stdlib only, `python3 -I`) that calls el tools as functions, so one call replaces many and only what the script prints enters context.

Searching code: use find for concepts and symbols, grep for exact strings and regex, and execute to batch the exploration (loops over grep/read, filtering output).

Behind execute: read, grep, edit, write, bash, background, find, websearch. Everything else answers through `tools.call("<name>", **kwargs)`, e.g. `tools.call("git", args=["status"])`.

import tools
print(tools.grep(pattern="TODO"))

tools.search(query) lists matching tools with signatures; tools.help(name) shows one. A failing tool call raises `tools.ToolError`, so catch it or the traceback returns.

MCP tools use fully qualified names in `tools.call` and `tools.parallel`, for example `tools.call('graph.search_graph', query='SessionRunner')` or `('graph.search_graph', {'query': 'SessionRunner'})`. Bare aliases such as `tools.search_graph(...)` work only as Python attributes when unique; the string `'search_graph'` is not a dispatch name. Discovery can include a cold server startup; do not repeat it for every query.

`tools.parallel([(name, kwargs), ...])` runs independent calls at once and returns results in order; edit and write are refused there because two writes can race on one file. `tools.save(name, code, description)` with `tools.run(name, **args)` keeps a script for a later call; `tools.store(key, value)` with `tools.load(key)` keeps JSON for this Session only. One script may make 500 tool calls.

Use execute when one result feeds the next, when you loop over many items, or when you need only a slice of big output. Independent calls: run them together with tools.parallel in one script, so the whole batch costs one model turn. A script with one tool call and no loop is a wasted turn: call that tool natively, or add the next independent call. Example, first look at code: print(*tools.parallel([("grep", {"pattern": "X"}), ("read", {"path": "a.rs"}), ("bash", {"command": "git status --short"})])).

Use `tools.edit(input=...)` for changes to existing files, including several sites or files; call edits sequentially, not through `tools.parallel`. Use `tools.write` for new files. Python file I/O is for analysis, not a shortcut around hashline validation.

Filter before you print. A script that prints a whole document, log or file only moves it into context. Compute the answer in the script and print that.

# loop: one call, not N
for p in ["TODO", "FIXME"]:
    print(p, tools.grep(pattern=p, max_results=5))

# filter: print only the answer
out = tools.bash(command="cargo test > /tmp/tests.log 2>&1; status=$?; tail -40 /tmp/tests.log; exit $status")
print(out)
