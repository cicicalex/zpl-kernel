# ZPL Kernel Trace Event Format

Status: draft v1 (kernel v0.3 era).

The kernel emits structured trace events over the COM1 serial port. Two
formats coexist for the v0.3 transition window, both deterministic and
parseable by `pb-cli trace-parse`.

## Format A — Legacy text markers (`v0`)

Pattern:

```
[ZPL-<TAG>] <free-form body>\n
```

Examples seen on the wire today:

```
[ZPL-BOOT] kernel_entry reached
[ZPL-BOOT] serial initialized
[ZPL-BOOT] halt loop entered
[ZPL-FRAME] init base=0x100000 cap=262144
[ZPL-FRAME] burst alloc=1000 free=1000 leak=0
[ZPL-FRAME] stress=1000000 leak=0
[ZPL-PAGING] init pdpt[1]+pd[0] ok
[ZPL-PAGING] map_4k virt=0x40000000 ok
[ZPL-PAGING] write_read_match val=0xcafebabedeadbeef
[ZPL-PAGING] unmap virt=0x40000000 ok
[ZPL-PAGING] page_fault virt=0x40000000 reread ok
[ZPL-SCHED tid=B ain=97 action=ALLOW]
[ZPL-PANIC] reason=test
[ZPL-PANIC] file=... line=... col=...
[ZPL-PANIC] rip=0x... rsp=0x... rflags=0x...
[ZPL-PANIC] qemu_exit=success
```

Properties:

- Always ASCII, terminated by `\n`.
- Tags are upper-case, `[A-Z0-9-]+`.
- Body is free-form text (interpreted ad-hoc by tests).

These markers MUST remain readable forever for replay of historical
boot logs.

## Format B — Structured event line (`v2`)

Canonical line:

```
[ZPL-EVT2 tag=<TAG> <key>=<value> ...]\n
```

Rules:

- Single line, ASCII, terminated by `\n`.
- `tag=` is the first key, identifies the event class (mirrors the legacy
  bracketed tag without the leading `ZPL-`).
- Each subsequent `key=value` pair is whitespace-separated. Values do not
  contain spaces; if a value would contain spaces, the producer escapes
  them as `_`.
- Keys are `[a-z][a-z0-9_]*`.
- Values are `[0-9a-zA-Z_:./-]+`. No quoting required.
- Order of keys is producer-defined; the parser MUST sort them when
  emitting structured output.

Reserved keys:

| Key       | Meaning |
|-----------|---------|
| `tag`     | event class, e.g. `BUILD-ID`, `BOOT`, `FRAME`, `PAGING`, `SCHED`, `PANIC` |
| `kind`    | sub-event within a class, e.g. `kernel_entry`, `map_4k`, `panic` |
| `t`       | event timestamp (u64). Boot-relative tick count when known, otherwise `0`. |
| `detail`  | optional human-readable detail string (no spaces) |

Example (emitted today):

```
[ZPL-EVT2 tag=BUILD-ID t=0 kind=kernel_image version=v0.3.0]
```

## Parser

`pb-cli trace-parse <log>` reads a COM1 boot log line by line and emits a
JSON array. Each entry is one of:

```json
{ "format": "legacy", "tag": "BOOT", "body": "kernel_entry reached" }
{ "format": "v2", "tag": "BUILD-ID", "kv": {"t": "0", "kind": "kernel_image", "version": "v0.3.0"} }
```

Lines that do not match either format are emitted as `{ "format": "raw",
"line": "<line>" }` so analysis tooling can locate parse gaps without
losing data.

## Stability guarantees

- Both formats are persisted forever; no "v1" intermediate has shipped.
- Adding new keys to `v2` is permitted — parsers ignore unknown keys.
- Renaming or removing existing keys is a breaking change and requires a
  new format tag (`ZPL-EVT3 ...`).
- The `tag` namespace is shared across kernel + userspace tools.

## Why two formats?

Backward compatibility: the existing QEMU smoke + determinism tests
already grep for the legacy markers. Replacing them in lock-step with
the kernel emit path would break the verification scripts
during the transition. Keeping both formats lets us migrate consumers
incrementally while the parser provides a single normalized view.
