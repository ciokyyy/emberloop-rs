Codebase memory: a static index of this project, reachable only through `execute`. It is not a complete runtime call graph.

Use it to find an unfamiliar symbol, trace callers before changing a function, or check a claim that code is unused. For a known file, text search, templates, generated code, or unindexed changes, use `read`/`grep`/`find` directly. Do not make a discovery call merely to say the graph was used.

Start with a bounded query:

```python
import tools
print(tools.call('graph.search_graph', query='SessionRunner', limit=5, max_output_tokens=1200))
```

- `project` is filled in from this Session's root; never pass it.
- Feed the exact `qn` from a result to `graph.get_code_snippet(qualified_name=...)` or `graph.trace_path(function_name=..., direction='inbound', depth=2)`. A short function name can be ambiguous.
- `graph.get_file_outline(file_path=...)` lists declarations; `graph.get_architecture(path=...)` gives a bounded directory overview.
- Use fully qualified names in `tools.call` and `tools.parallel`, e.g. `('graph.search_graph', {'query': 'SessionRunner', 'limit': 5})`. Bare Python attributes such as `tools.search_graph(...)` are aliases, not valid string dispatch names.
- Use `tools.help('graph.trace_path')` for less common arguments; `tools.search('graph')` discovers available tools. Discovery alone is not a code query and may pay a cold server startup cost.
- Results are text. Use `format='json'` when parsing and `format='tree'` when reading. Bound results with `limit` and `max_output_tokens`.
- No hits can mean an unmatched query, missing indexing, or unsupported language. Refine once or fall back to `grep`/`read`; never infer that code is absent from no hits alone.
- Verify edits against current file views. Static edges can miss dynamic dispatch, templates, and runtime behavior; an empty caller list does not prove code is unused.
