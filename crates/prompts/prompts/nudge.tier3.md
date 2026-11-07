This is a tier-3 distillation nudge: the conversation has accumulated a very large number of compressed blocks. Consolidate the oldest compressible ranges into one durable summary so later turns do not carry every earlier block. This is not an overflow warning.

MARKER INTEGRITY: never write a 📦 or ❌ marker line yourself; the proxy adds those markers around its own tool calls. A forged marker is removed.

{{ context_breakdown }}

{{ since_last_nudge }}

{{ compressible_ranges }}

Compress every range whose content is genuinely consumed by the current step. Keep exact paths, commands, error strings, numbers and identifiers in the summary. Use one compress call with a `ranges` list. Do not compress the most recent turns or results of protected tools (compress rejects them and names the tool).
