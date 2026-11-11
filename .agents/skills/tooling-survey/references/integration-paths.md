# Integration paths in emberloop-rs

How a candidate tool can actually be reached from a Session. This decides the
ranking before any benchmark matters.

| Path | Reached from | Cost model |
|---|---|---|
| MCP stdio server | `execute`, as `tools.<server>.<tool>(...)` | schemas stay out of the model tool list; cost is the output plus one `tools.search`/`help` call |
| CLI binary | `execute` or `bash` | cost is the output; the model learns the flags from `--help` or a skill |
| HTTP API | `execute` (`read` for GET) | cost is the response body |
| Built-in tool | the tool list | schema is loaded on every call, so its size is a per-call cost |

The MCP registry reads `<config root>/mcp.json`, stdio only; a `url` entry is
refused. It starts one child per server on first use and kills it when the
Session's tool server drops, so a server a Session uses does not outlive it.

Consequence for surveys: a tool shipping both an MCP server and a CLI has the
same call-time cost here. Rank it on output shape and process footprint, not
on the protocol.
